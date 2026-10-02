package com.petstore;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import okhttp3.Interceptor;
import okhttp3.MediaType;
import okhttp3.Protocol;
import okhttp3.Request;
import okhttp3.Response;
import okhttp3.ResponseBody;
import org.junit.jupiter.api.Test;

class MiddlewareTest {
    private static final String PET = "{\"id\":\"1\",\"name\":\"Rex\",\"created_at\":\"2024-01-01T00:00:00Z\"}";

    private static Response respond(Request request, String body) {
        return new Response.Builder()
                .request(request)
                .protocol(Protocol.HTTP_1_1)
                .code(200)
                .message("OK")
                .body(ResponseBody.create(body, MediaType.parse("application/json")))
                .build();
    }

    private static Interceptor origin(List<Request> seen) {
        return chain -> {
            seen.add(chain.request());
            return respond(chain.request(), PET);
        };
    }

    private static Interceptor cache() {
        Map<String, String> store = new HashMap<>();
        return chain -> {
            Request request = chain.request();
            if (!request.method().equals("GET")) {
                return chain.proceed(request);
            }
            String key = request.url().toString();
            if (!store.containsKey(key)) {
                Response response = chain.proceed(request);
                if (!response.isSuccessful()) {
                    return response;
                }
                store.put(key, response.body().string());
            }
            return respond(request, store.get(key));
        };
    }

    @Test
    void cacheAnswersRepeatedGetsWithoutReachingTheOrigin() {
        List<Request> seen = new ArrayList<>();
        PetstoreOptions options = PetstoreOptions.builder().addInterceptor(cache()).addInterceptor(origin(seen)).build();
        try (Petstore petstore = new Petstore("token", options)) {
            for (int i = 0; i < 3; i++) {
                assertEquals("Rex", petstore.pets().retrieve("1").name());
            }
            assertEquals(1, seen.size());
            petstore.pets().retrieve("2");
            assertEquals(2, seen.size());
        }
    }

    @Test
    void middlewareCanChangeTheRequest() {
        List<Request> seen = new ArrayList<>();
        PetstoreOptions options = PetstoreOptions.builder()
                .addInterceptor(chain -> chain.proceed(chain.request().newBuilder().header("x-tag", "yes").build()))
                .addInterceptor(origin(seen))
                .header("x-default", "d")
                .build();
        new Petstore("token", options).pets().retrieve("1");

        assertEquals("yes", seen.get(0).header("x-tag"));
        assertEquals("d", seen.get(0).header("x-default"));
        assertEquals("Bearer token", seen.get(0).header("Authorization"));
    }

    @Test
    void optionsAreImmutable() {
        PetstoreOptions options = PetstoreOptions.builder().addInterceptor(chain -> chain.proceed(chain.request())).build();
        assertThrows(UnsupportedOperationException.class, () -> options.interceptors().clear());
        PetstoreOptions changed = options.toBuilder().maxRetries(0).build();
        assertTrue(changed.retrySchedule().isEmpty());
        assertEquals(2, options.retrySchedule().size());
    }
}
