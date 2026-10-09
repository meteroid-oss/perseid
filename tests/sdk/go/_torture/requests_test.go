package torture

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"mime"
	"mime/multipart"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestQueryAndJSONBodyOptions(t *testing.T) {
	client, rec := server(t, func(_ int32, w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		w.Header().Set("X-Body", string(body))
		_, _ = io.WriteString(w, thingJSON)
	})
	var resp *http.Response
	_, err := client.Things().Create(context.Background(), ThingCreate{Name: "n", Kind: KindAlpha}, nil,
		WithQuery("beta", "1"), WithJSONSet("name", "renamed"), WithJSONSet("extra.deep.flag", true), WithResponseInto(&resp))
	if err != nil {
		t.Fatal(err)
	}
	if got := rec.last.Load().URL.Query().Get("beta"); got != "1" {
		t.Errorf("beta = %q", got)
	}
	var body map[string]json.RawMessage
	if err := json.Unmarshal([]byte(resp.Header.Get("X-Body")), &body); err != nil {
		t.Fatal(err)
	}
	if string(body["name"]) != `"renamed"` || string(body["kind"]) != `"alpha"` || string(body["extra"]) != `{"deep":{"flag":true}}` {
		t.Errorf("body = %s", resp.Header.Get("X-Body"))
	}

	_, err = client.Things().Create(context.Background(), ThingCreate{Name: "n", Kind: KindAlpha}, nil, WithJSONSet("name.first", "x"))
	var reqErr *RequestError
	if !errors.As(err, &reqErr) {
		t.Errorf("setting into a string: %v", err)
	}
}

func multipartFiles(t *testing.T, req *request) map[string][2]string {
	t.Helper()
	_, params, _ := mime.ParseMediaType(req.contentType)
	body, err := req.newBody()
	if err != nil {
		t.Fatal(err)
	}
	files := map[string][2]string{}
	reader := multipart.NewReader(body, params["boundary"])
	for {
		part, err := reader.NextPart()
		if errors.Is(err, io.EOF) {
			return files
		}
		if err != nil {
			t.Fatal(err)
		}
		content, _ := io.ReadAll(part)
		files[part.FileName()] = [2]string{part.Header.Get("Content-Type"), string(content)}
	}
}

func TestUploadsAreNamedAfterTheirFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "notes.txt")
	if err := os.WriteFile(path, []byte("hello"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "data.bin"), []byte("bytes"), 0o600); err != nil {
		t.Fatal(err)
	}
	opened, err := os.Open(filepath.Join(dir, "data.bin"))
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = opened.Close() }()
	fromPath, err := UploadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	req := newRequest(http.MethodPost, "/files", nil)
	req.SetMultipartBody([]multipartField{
		{name: "opened", file: &Upload{Reader: opened}},
		{name: "path", files: []Upload{fromPath}},
		{name: "named", file: &Upload{Reader: strings.NewReader("x"), Filename: "given.json"}},
		{name: "generic", file: &Upload{Reader: strings.NewReader("y"), Filename: "generic.json"}, contentType: "application/octet-stream"},
		{name: "declared", file: &Upload{Reader: strings.NewReader("z"), Filename: "declared.json"}, contentType: "image/png"},
	})
	if req.oneShot {
		t.Error("files are not replayable")
	}
	for attempt := range 2 {
		files := multipartFiles(t, req)
		if files["data.bin"] != [2]string{"application/octet-stream", "bytes"} || files["given.json"] != [2]string{"application/json", "x"} ||
			files["generic.json"] != [2]string{"application/json", "y"} || files["declared.json"] != [2]string{"image/png", "z"} ||
			files["notes.txt"][1] != "hello" || !strings.HasPrefix(files["notes.txt"][0], "text/plain") {
			t.Errorf("attempt %d: %v", attempt, files)
		}
	}

	if _, err := UploadFile(filepath.Join(t.TempDir(), "missing")); !errors.Is(err, os.ErrNotExist) {
		t.Errorf("missing file: %v", err)
	}
}
