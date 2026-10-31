package main

import (
	"archive/zip"
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func testToken(scope string) string {
	p := base64.RawURLEncoding.EncodeToString([]byte(`{"scp":"` + scope + `"}`))
	return "header." + p + ".signature"
}

func TestBackendIDsRequireResultsScope(t *testing.T) {
	run, job, err := backendIDs(testToken("Actions.Other:x:y Actions.Results:run-1:job_2"))
	if err != nil || run != "run-1" || job != "job_2" {
		t.Fatalf("unexpected IDs %q %q %v", run, job, err)
	}
	if _, _, err = backendIDs(testToken("Actions.Results:run/../other:job")); err == nil {
		t.Fatal("accepted unsafe scope IDs")
	}
	if _, _, err = backendIDs("super-secret-token"); err == nil {
		t.Fatal("accepted malformed token")
	}
}

func TestReadInputAndWorkspacePathFailures(t *testing.T) {
	get := func(values map[string]string) func(string) string { return func(k string) string { return values[k] } }
	if _, err := readInput(get(map[string]string{"INPUT_NAME": "x", "INPUT_PATH": "a", "INPUT_RETENTION-DAYS": "91"})); err == nil {
		t.Fatal("accepted excessive retention")
	}
	if _, err := readInput(get(map[string]string{"INPUT_NAME": "unsafe:name", "INPUT_PATH": "a"})); err == nil {
		t.Fatal("accepted invalid artifact name")
	}
	clamped, err := readInput(get(map[string]string{"INPUT_NAME": "x", "INPUT_PATH": "a", "INPUT_RETENTION-DAYS": "7", "GITHUB_RETENTION_DAYS": "5"}))
	if err != nil || clamped.retentionDays != 5 {
		t.Fatalf("repository retention cap was not applied: %#v %v", clamped, err)
	}
	if _, err := readInput(get(map[string]string{"INPUT_NAME": "x", "INPUT_PATH": ""})); err == nil {
		t.Fatal("accepted missing path")
	}
	if in, err := readInput(get(map[string]string{"INPUT_NAME": "x", "INPUT_PATH": "a", "INPUT_INCLUDE-HIDDEN-FILES": "true"})); err != nil || !in.includeHidden {
		t.Fatalf("failed to parse include-hidden-files: %#v %v", in, err)
	}
	root := t.TempDir()
	if _, _, err := collectInput(root, "../outside", false); err == nil {
		t.Fatal("accepted path traversal")
	}
	if _, _, err := collectInput(root, "missing", false); err == nil {
		t.Fatal("accepted missing path")
	}
	empty := filepath.Join(root, "empty")
	if err := os.Mkdir(empty, 0700); err != nil {
		t.Fatal(err)
	}
	files, _, err := collectInput(root, "empty", false)
	if err != nil || len(files) != 0 {
		t.Fatalf("unexpected empty directory result: %v %d", err, len(files))
	}
	values := map[string]string{"INPUT_NAME": "none", "INPUT_PATH": "empty", "GITHUB_WORKSPACE": root}
	if err := run(context.Background(), get(values), io.Discard); err == nil || !strings.Contains(err.Error(), "contains no files") {
		t.Fatalf("expected clear no-files failure before runtime setup; got %v", err)
	}
}

func TestCollectInputRejectsSymlinks(t *testing.T) {
	root := t.TempDir()
	outside := filepath.Join(t.TempDir(), "secret")
	if err := os.WriteFile(outside, []byte("secret"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(root, "leak")); err != nil {
		t.Skipf("symlink unavailable: %v", err)
	}
	if _, _, err := collectInput(root, "leak", false); err == nil {
		t.Fatal("accepted symlink file")
	}
}

func TestHiddenTransportDirectoryRequiresExplicitOptIn(t *testing.T) {
	root := t.TempDir()
	dir := filepath.Join(root, ".artifact-transport")
	if err := os.Mkdir(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "bundle.zip"), []byte("zip"), 0600); err != nil {
		t.Fatal(err)
	}
	if _, _, err := collectInput(root, ".artifact-transport", false); err == nil {
		t.Fatal("hidden transfer directory accepted without opt-in")
	}
	files, archiveRoot, err := collectInput(root, ".artifact-transport", true)
	if err != nil || len(files) != 1 {
		t.Fatalf("hidden transfer directory rejected with opt-in: %v files=%d", err, len(files))
	}
	resolvedDir, _ := filepath.EvalSymlinks(dir)
	if archiveRoot != resolvedDir || files[0].archiveName != "bundle.zip" {
		t.Fatalf("unexpected hidden-root archive path: %q %q", archiveRoot, files[0].archiveName)
	}
}

func TestArchivePreservesExpectedRelativeNamesAndBytes(t *testing.T) {
	root := t.TempDir()
	dir := filepath.Join(root, "artifacts")
	if err := os.Mkdir(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "receipt.json"), []byte(`{"safe":true}`), 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, ".secret"), []byte("hidden"), 0600); err != nil {
		t.Fatal(err)
	}
	files, archiveRoot, err := collectInput(root, "artifacts", false)
	if err != nil {
		t.Fatal(err)
	}
	archive, size, err := createArchive(files, archiveRoot)
	if err != nil {
		t.Fatal(err)
	}
	defer os.Remove(archive)
	if size == 0 {
		t.Fatal("empty archive")
	}
	z, err := zip.OpenReader(archive)
	if err != nil {
		t.Fatal(err)
	}
	defer z.Close()
	if len(z.File) != 1 || z.File[0].Name != "receipt.json" {
		t.Fatalf("unexpected archive names: %#v", z.File)
	}
	r, err := z.File[0].Open()
	if err != nil {
		t.Fatal(err)
	}
	b, _ := io.ReadAll(r)
	r.Close()
	if string(b) != `{"safe":true}` {
		t.Fatalf("archive bytes changed: %q", b)
	}
}

func TestUploadProtocolDoesNotLeakTokensOrSignedURL(t *testing.T) {
	const token = "runtime-token-do-not-log"
	var create, finalize map[string]any
	var blocks int
	var blockList string
	server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/twirp/" + serviceName + "/CreateArtifact":
			if r.Header.Get("Authorization") != "Bearer "+token {
				t.Errorf("missing bearer auth")
			}
			json.NewDecoder(r.Body).Decode(&create)
			json.NewEncoder(w).Encode(map[string]any{"ok": true, "signedUploadUrl": serverURL(r) + "/blob?sig=do-not-log"})
		case "/blob":
			if r.URL.Query().Get("comp") == "block" {
				blocks++
				io.Copy(io.Discard, r.Body)
				w.WriteHeader(201)
				return
			}
			if r.URL.Query().Get("comp") == "blocklist" {
				b, _ := io.ReadAll(r.Body)
				blockList = string(b)
				w.WriteHeader(201)
				return
			}
			http.Error(w, "bad blob operation", 400)
		case "/twirp/" + serviceName + "/FinalizeArtifact":
			json.NewDecoder(r.Body).Decode(&finalize)
			json.NewEncoder(w).Encode(map[string]bool{"ok": true})
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()
	archive := filepath.Join(t.TempDir(), "archive.zip")
	payload := strings.Repeat("x", blockSize+3)
	if err := os.WriteFile(archive, []byte(payload), 0600); err != nil {
		t.Fatal(err)
	}
	u := &uploader{client: server.Client(), resultsURL: server.URL, token: token, runBackend: "run", jobBackend: "job", now: func() time.Time { return time.Date(2026, 10, 31, 0, 0, 0, 0, time.UTC) }}
	err := u.upload(context.Background(), "native-check", 7, archive, int64(len(payload)))
	if err != nil {
		t.Fatal(err)
	}
	if blocks != 2 || !strings.Contains(blockList, "<BlockList>") {
		t.Fatalf("incomplete Azure block upload: blocks=%d list=%q", blocks, blockList)
	}
	if create["version"] != float64(4) || create["name"] != "native-check" {
		t.Fatalf("unexpected CreateArtifact request: %#v", create)
	}
	if create["expiresAt"] != "2026-11-07T00:00:00Z" {
		t.Fatalf("unexpected retention timestamp: %#v", create["expiresAt"])
	}
	if finalize["size"] != strconvItoa(len(payload)) || finalize["name"] != "native-check" {
		t.Fatalf("unexpected FinalizeArtifact request: %#v", finalize)
	}
	wantHash := sha256.Sum256([]byte(payload))
	hashField, ok := finalize["hash"].(map[string]any)
	if !ok || hashField["value"] != "sha256:"+hex.EncodeToString(wantHash[:]) {
		t.Fatalf("missing archive digest in finalization: %#v", finalize["hash"])
	}
	log := &strings.Builder{}
	if strings.Contains(log.String(), token) || strings.Contains(log.String(), "do-not-log") {
		t.Fatal("secret in output")
	}
}

func TestArtifactResponsesAcceptProtoAndCamelCaseFieldNames(t *testing.T) {
	for _, tc := range []struct {
		name       string
		response   string
		wantURL    string
		wantIDJSON string
	}{
		{
			name:       "camelCase",
			response:   `{"ok":true,"signedUploadUrl":"https://blob.example/upload?sig=redacted","artifactId":"123"}`,
			wantURL:    "https://blob.example/upload?sig=redacted",
			wantIDJSON: `"123"`,
		},
		{
			name:       "protoFieldNames",
			response:   `{"ok":true,"signed_upload_url":"https://blob.example/upload?sig=redacted","artifact_id":"456"}`,
			wantURL:    "https://blob.example/upload?sig=redacted",
			wantIDJSON: `"456"`,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			var created createArtifactResponse
			if err := json.Unmarshal([]byte(tc.response), &created); err != nil {
				t.Fatal("could not decode CreateArtifact response")
			}
			var finalized finalizeArtifactResponse
			if err := json.Unmarshal([]byte(tc.response), &finalized); err != nil {
				t.Fatal("could not decode FinalizeArtifact response")
			}
			if !created.OK || created.uploadURL() != tc.wantURL {
				t.Fatal("CreateArtifact response fields were not decoded")
			}
			if !finalized.OK || string(finalized.artifactID()) != tc.wantIDJSON {
				t.Fatal("FinalizeArtifact response fields were not decoded")
			}
		})
	}
}

func TestParseHTTPSBaseUsesOriginAndRejectsPlainHTTP(t *testing.T) {
	origin, err := parseHTTPSBase("https://results.example.test/some/path?ignored=no")
	if err != nil || origin != "https://results.example.test" {
		t.Fatalf("unexpected service origin %q: %v", origin, err)
	}
	if _, err = parseHTTPSBase("http://results.example.test"); err == nil {
		t.Fatal("accepted non-TLS Results URL")
	}
}

func TestTwirpErrorsDoNotExposeResponseBody(t *testing.T) {
	const secret = "runtime-token-do-not-log"
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, secret, http.StatusForbidden)
	}))
	defer server.Close()
	u := &uploader{client: server.Client(), resultsURL: server.URL, token: secret}
	err := u.twirp(context.Background(), "CreateArtifact", map[string]any{}, &map[string]any{})
	if err == nil || strings.Contains(err.Error(), secret) {
		t.Fatalf("response body leaked in error: %v", err)
	}
}

func serverURL(r *http.Request) string { return "https://" + r.Host }
func strconvItoa(n int) string         { return fmt.Sprintf("%d", n) }
