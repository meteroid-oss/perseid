package vault

import (
	"context"
	"encoding/base64"
	"errors"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

// The OAuth2 client credentials flow of the SDK generated from tests/fixtures/oauth.yaml, against
// a fake transport that plays the token endpoint and the API.

const (
	baseURL  = "https://vault.test/v1"
	tokenURL = baseURL + "/oauth/token"
)

// server issues the access tokens at-1, at-2... and rejects those in revoked with a 401.
type server struct {
	mu        sync.Mutex
	expiresIn string
	revoked   map[string]bool
	scripted  []*http.Response
	issued    int
	requests  []*http.Request
	bodies    []string
}

func newServer() *server { return &server{expiresIn: "3600", revoked: map[string]bool{}} }

func response(req *http.Request, status int, body string) *http.Response {
	return &http.Response{
		StatusCode: status,
		Header:     http.Header{"Content-Type": {"application/json"}},
		Body:       io.NopCloser(strings.NewReader(body)),
		Request:    req,
	}
}

func (s *server) RoundTrip(req *http.Request) (*http.Response, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	body := ""
	if req.Body != nil {
		raw, _ := io.ReadAll(req.Body)
		body = string(raw)
	}
	s.requests = append(s.requests, req)
	s.bodies = append(s.bodies, body)
	if req.URL.String() == tokenURL {
		if len(s.scripted) > 0 {
			next := s.scripted[0]
			s.scripted = s.scripted[1:]
			next.Request = req
			return next, nil
		}
		s.issued++
		return response(req, 200, `{"access_token":"at-`+strconv.Itoa(s.issued)+`","token_type":"Bearer","expires_in":`+s.expiresIn+`}`), nil
	}
	token := strings.TrimPrefix(req.Header.Get("Authorization"), "Bearer ")
	if s.revoked[token] {
		return response(req, 401, `{"message":"revoked"}`), nil
	}
	if token == "" {
		token = "anonymous"
	}
	return response(req, 200, `{"status":"`+token+`"}`), nil
}

func (s *server) tokenRequests() (requests []*http.Request, bodies []string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i, req := range s.requests {
		if req.URL.String() == tokenURL {
			requests, bodies = append(requests, req), append(bodies, s.bodies[i])
		}
	}
	return requests, bodies
}

func (s *server) total() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return len(s.requests)
}

// client is a client with credentials, whose transport is s.
func (s *server) client(options Options) *Client {
	options.Middleware = append(options.Middleware, func(http.RoundTripper) http.RoundTripper { return s })
	options.ServerURL = baseURL
	if options.ClientID == "" {
		options.ClientID, options.ClientSecret = "id", "secret"
	}
	if options.RetrySchedule == nil {
		options.MaxRetries = -1
	}
	return New("", &options)
}

func basic(id, secret string) string {
	return "Basic " + base64.StdEncoding.EncodeToString([]byte(id+":"+secret))
}

func status(t *testing.T, client *Client) string {
	t.Helper()
	health, err := client.Account().RetrieveMachine(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	return health.Status
}

func want(t *testing.T, got, want any) {
	t.Helper()
	if got != want {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func TestATokenIsFetchedOnFirstUseAndKept(t *testing.T) {
	t.Setenv(APIKeyEnv, "")
	s := newServer()
	client := s.client(Options{})
	want(t, status(t, client), "at-1")
	want(t, status(t, client), "at-1")
	tokens, bodies := s.tokenRequests()
	want(t, len(tokens), 1)
	want(t, tokens[0].Method, http.MethodPost)
	want(t, tokens[0].Header.Get("Content-Type"), "application/x-www-form-urlencoded")
	want(t, tokens[0].Header.Get("Authorization"), basic("id", "secret"))
	want(t, bodies[0], "grant_type=client_credentials&scope=secrets.read+secrets.write")
	want(t, s.requests[1].Header.Get("Authorization"), "Bearer at-1")
}

func TestPublicOperationsSendNoCredentialsAndFetchNoToken(t *testing.T) {
	t.Setenv(APIKeyEnv, "")
	s := newServer()
	client := s.client(Options{})
	health, err := client.Account().CheckHealth(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	want(t, health.Status, "anonymous")
	want(t, s.total(), 1)
	session, err := client.Account().CreateSession(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	want(t, session.Status, "at-1")
}

func TestCredentialsAreFormEncodedBeforeTheBasicHeaderOrSentInTheBody(t *testing.T) {
	t.Setenv(APIKeyEnv, "")
	s := newServer()
	status(t, s.client(Options{ClientID: "a b", ClientSecret: "p@ss:word"}))
	tokens, _ := s.tokenRequests()
	want(t, tokens[0].Header.Get("Authorization"), basic("a+b", "p%40ss%3Aword"))

	s = newServer()
	status(t, s.client(Options{OAuthClientAuth: "body"}))
	tokens, bodies := s.tokenRequests()
	want(t, tokens[0].Header.Get("Authorization"), "")
	form, err := url.ParseQuery(bodies[0])
	if err != nil {
		t.Fatal(err)
	}
	want(t, form.Get("client_id")+"/"+form.Get("client_secret"), "id/secret")
}

func TestAnExpiredTokenIsRenewed(t *testing.T) {
	t.Setenv(APIKeyEnv, "")
	s := newServer()
	s.expiresIn = "0"
	client := s.client(Options{})
	want(t, status(t, client), "at-1")
	want(t, status(t, client), "at-2")
	tokens, _ := s.tokenRequests()
	want(t, len(tokens), 2)
}

func TestATokenTheAPIRejectsIsReplacedOnce(t *testing.T) {
	t.Setenv(APIKeyEnv, "")
	s := newServer()
	s.revoked["at-1"] = true
	client := s.client(Options{})
	want(t, status(t, client), "at-2")
	tokens, _ := s.tokenRequests()
	want(t, len(tokens), 2)
	want(t, s.total()-len(tokens), 2)
	want(t, status(t, client), "at-2")
	tokens, _ = s.tokenRequests()
	want(t, len(tokens), 2)

	s = newServer()
	s.revoked["at-1"], s.revoked["at-2"], s.revoked["at-3"] = true, true, true
	_, err := s.client(Options{}).Account().RetrieveMachine(context.Background())
	if !errors.Is(err, ErrUnauthorized) {
		t.Fatalf("err = %v, want a 401", err)
	}
	tokens, _ = s.tokenRequests()
	want(t, len(tokens), 2)
}

func TestTheTokenRequestIsRetriedLikeAnyRequest(t *testing.T) {
	t.Setenv(APIKeyEnv, "")
	s := newServer()
	s.scripted = []*http.Response{response(nil, 503, `{"error":"busy"}`)}
	client := s.client(Options{RetrySchedule: []time.Duration{0}})
	want(t, status(t, client), "at-1")
	tokens, _ := s.tokenRequests()
	want(t, len(tokens), 2)

	s = newServer()
	s.scripted = []*http.Response{response(nil, 401, `{"error":"invalid_client"}`)}
	_, err := s.client(Options{}).Account().RetrieveMachine(context.Background())
	var apiErr *APIError
	if !errors.As(err, &apiErr) || apiErr.StatusCode != 401 {
		t.Fatalf("err = %v, want a 401", err)
	}
	want(t, s.total(), 1)

	s = newServer()
	s.scripted = []*http.Response{response(nil, 200, `{"token_type":"Bearer"}`)}
	_, err = s.client(Options{}).Account().RetrieveMachine(context.Background())
	var decode *DecodeError
	if !errors.As(err, &decode) {
		t.Fatalf("err = %v, want a DecodeError", err)
	}
}

func TestConcurrentCallsShareOneTokenRequest(t *testing.T) {
	t.Setenv(APIKeyEnv, "")
	s := newServer()
	client := s.client(Options{})
	var wg sync.WaitGroup
	for range 5 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if got := status(t, client); got != "at-1" {
				t.Errorf("got %q", got)
			}
		}()
	}
	wg.Wait()
	tokens, _ := s.tokenRequests()
	want(t, len(tokens), 1)
}

func TestATokenOrAProviderWinsOverTheClientCredentials(t *testing.T) {
	t.Setenv(APIKeyEnv, "")
	s := newServer()
	client := s.client(Options{TokenProvider: func(context.Context) (string, error) { return "mine", nil }})
	want(t, status(t, client), "mine")
	static := s.client(Options{}).WithToken("static")
	want(t, status(t, static), "static")
	tokens, _ := s.tokenRequests()
	want(t, len(tokens), 0)
}

func TestTheCredentialsDefaultToTheEnvironment(t *testing.T) {
	t.Setenv(APIKeyEnv, "")
	t.Setenv(ClientIDEnv, "env-id")
	t.Setenv(ClientSecretEnv, "env-secret")
	s := newServer()
	options := Options{
		ServerURL:  baseURL,
		MaxRetries: -1,
		Middleware: []Middleware{func(http.RoundTripper) http.RoundTripper { return s }},
	}
	want(t, status(t, New("", &options)), "at-1")
	tokens, _ := s.tokenRequests()
	want(t, tokens[0].Header.Get("Authorization"), basic("env-id", "env-secret"))
}

func TestARelativeTokenURLIsResolvedAgainstTheServerURL(t *testing.T) {
	t.Setenv(APIKeyEnv, "")
	s := newServer()
	forward := func(http.RoundTripper) http.RoundTripper {
		return RoundTripperFunc(func(req *http.Request) (*http.Response, error) {
			moved := req.Clone(req.Context())
			moved.URL, _ = url.Parse(strings.Replace(req.URL.String(), "https://other.test/api", baseURL, 1))
			return s.RoundTrip(moved)
		})
	}
	client := New("", &Options{ServerURL: "https://other.test/api/", ClientID: "id", ClientSecret: "secret", Middleware: []Middleware{forward}})
	want(t, status(t, client), "at-1")
	want(t, s.requests[0].URL.String(), tokenURL)
}
