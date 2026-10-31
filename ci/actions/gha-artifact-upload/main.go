package main

import (
	"archive/zip"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"
)

const (
	serviceName = "github.actions.results.api.v1.ArtifactService"
	maxFiles    = 100000
	blockSize   = 8 * 1024 * 1024
)

type uploader struct {
	client     *http.Client
	resultsURL string
	token      string
	runBackend string
	jobBackend string
	workspace  string
	now        func() time.Time
}

type actionInput struct {
	name          string
	path          string
	retentionDays int
	includeHidden bool
}

type createArtifactResponse struct {
	OK                   bool   `json:"ok"`
	SignedUploadURL      string `json:"signedUploadUrl"`
	SignedUploadURLSnake string `json:"signed_upload_url"`
}

func (r createArtifactResponse) uploadURL() string {
	if r.SignedUploadURL != "" {
		return r.SignedUploadURL
	}
	return r.SignedUploadURLSnake
}

type finalizeArtifactResponse struct {
	OK              bool            `json:"ok"`
	ArtifactID      json.RawMessage `json:"artifactId"`
	ArtifactIDSnake json.RawMessage `json:"artifact_id"`
}

func (r finalizeArtifactResponse) artifactID() json.RawMessage {
	if len(r.ArtifactID) != 0 {
		return r.ArtifactID
	}
	return r.ArtifactIDSnake
}

func main() {
	if err := run(context.Background(), os.Getenv, os.Stdout); err != nil {
		fmt.Fprintln(os.Stderr, "artifact upload failed:", err)
		os.Exit(1)
	}
}

func run(ctx context.Context, getenv func(string) string, out io.Writer) error {
	in, err := readInput(getenv)
	if err != nil {
		return err
	}
	workspace := getenv("GITHUB_WORKSPACE")
	if workspace == "" {
		return errors.New("GITHUB_WORKSPACE is required")
	}
	workspace, err = filepath.Abs(workspace)
	if err != nil {
		return errors.New("invalid workspace")
	}
	workspace, err = filepath.EvalSymlinks(workspace)
	if err != nil {
		return errors.New("workspace is unavailable")
	}
	files, archiveRoot, err := collectInput(workspace, in.path, in.includeHidden)
	if err != nil {
		return err
	}
	if len(files) == 0 {
		return errors.New("path contains no files")
	}
	runID, jobID, err := backendIDs(getenv("ACTIONS_RUNTIME_TOKEN"))
	if err != nil {
		return err
	}
	resultsURL, err := parseHTTPSBase(getenv("ACTIONS_RESULTS_URL"))
	if err != nil {
		return errors.New("ACTIONS_RESULTS_URL is missing or invalid")
	}
	u := &uploader{client: &http.Client{Timeout: 10 * time.Minute, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}, resultsURL: resultsURL, token: getenv("ACTIONS_RUNTIME_TOKEN"), runBackend: runID, jobBackend: jobID, workspace: workspace, now: time.Now}
	archive, size, err := createArchive(files, archiveRoot)
	if err != nil {
		return err
	}
	defer os.Remove(archive)
	if err = u.upload(ctx, in.name, in.retentionDays, archive, size); err != nil {
		return err
	}
	fmt.Fprintf(out, "Uploaded artifact %q (%d files, %d bytes).\n", in.name, len(files), size)
	return nil
}

func readInput(getenv func(string) string) (actionInput, error) {
	in := actionInput{name: strings.TrimSpace(getenv("INPUT_NAME")), path: strings.TrimSpace(getenv("INPUT_PATH"))}
	if in.name == "" || len(in.name) > 256 || strings.ContainsAny(in.name, "\\/:<>|*?\"\r\n") {
		return in, errors.New("artifact name is empty or invalid")
	}
	if in.path == "" {
		return in, errors.New("path is required")
	}
	includeHidden := strings.TrimSpace(getenv("INPUT_INCLUDE-HIDDEN-FILES"))
	if includeHidden != "" {
		value, err := strconv.ParseBool(includeHidden)
		if err != nil {
			return in, errors.New("include-hidden-files must be true or false")
		}
		in.includeHidden = value
	}
	if days := strings.TrimSpace(getenv("INPUT_RETENTION-DAYS")); days != "" {
		value, err := strconv.Atoi(days)
		if err != nil || value < 1 || value > 90 {
			return in, errors.New("retention-days must be between 1 and 90")
		}
		in.retentionDays = value
	}
	if in.retentionDays > 0 {
		if maximum, err := strconv.Atoi(strings.TrimSpace(getenv("GITHUB_RETENTION_DAYS"))); err == nil && maximum > 0 && maximum < in.retentionDays {
			in.retentionDays = maximum
		}
	}
	return in, nil
}

func parseHTTPSBase(raw string) (string, error) {
	u, err := url.Parse(strings.TrimSpace(raw))
	if err != nil || u.Scheme != "https" || u.Host == "" || u.User != nil {
		return "", errors.New("invalid URL")
	}
	return u.Scheme + "://" + u.Host, nil
}

func backendIDs(token string) (string, string, error) {
	parts := strings.Split(token, ".")
	if len(parts) != 3 || token == "" {
		return "", "", errors.New("runtime token is missing or invalid")
	}
	data, err := base64.RawURLEncoding.DecodeString(parts[1])
	if err != nil {
		data, err = base64.URLEncoding.DecodeString(parts[1])
	}
	if err != nil {
		return "", "", errors.New("runtime token is missing or invalid")
	}
	var claims struct {
		Scope string `json:"scp"`
	}
	if json.Unmarshal(data, &claims) != nil {
		return "", "", errors.New("runtime token is missing or invalid")
	}
	for _, scope := range strings.Fields(claims.Scope) {
		p := strings.Split(scope, ":")
		if len(p) == 3 && p[0] == "Actions.Results" && safeBackendID(p[1]) && safeBackendID(p[2]) {
			return p[1], p[2], nil
		}
	}
	return "", "", errors.New("runtime token lacks artifact scope")
}

func safeBackendID(s string) bool {
	if len(s) < 1 || len(s) > 128 {
		return false
	}
	for _, r := range s {
		if !(r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || r == '-' || r == '_') {
			return false
		}
	}
	return true
}

type sourceFile struct {
	full, archiveName string
	info              os.FileInfo
}

func collectInput(workspace, raw string, includeHidden bool) ([]sourceFile, string, error) {
	workspace, err := filepath.Abs(workspace)
	if err != nil {
		return nil, "", errors.New("invalid workspace")
	}
	workspace, err = filepath.EvalSymlinks(workspace)
	if err != nil {
		return nil, "", errors.New("workspace is unavailable")
	}
	if filepath.IsAbs(raw) {
		return nil, "", errors.New("path must be workspace-relative")
	}
	clean := filepath.Clean(raw)
	if clean == ".." || strings.HasPrefix(clean, ".."+string(filepath.Separator)) {
		return nil, "", errors.New("path escapes workspace")
	}
	target := filepath.Join(workspace, clean)
	resolved, err := filepath.EvalSymlinks(target)
	if err != nil {
		return nil, "", errors.New("path does not exist")
	}
	rel, err := filepath.Rel(workspace, resolved)
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return nil, "", errors.New("path escapes workspace")
	}
	if !includeHidden {
		for _, segment := range strings.Split(filepath.ToSlash(rel), "/") {
			if strings.HasPrefix(segment, ".") {
				return nil, "", errors.New("hidden paths are excluded unless include-hidden-files is true")
			}
		}
	}
	st, err := os.Lstat(target)
	if err != nil || st.Mode()&os.ModeSymlink != 0 {
		return nil, "", errors.New("symlink paths are not allowed")
	}
	root := filepath.Dir(resolved)
	if st.IsDir() {
		root = resolved
	} else if !st.Mode().IsRegular() {
		return nil, "", errors.New("path must be a regular file or directory")
	}
	files := []sourceFile{}
	add := func(p string, info os.FileInfo) error {
		if len(files) >= maxFiles {
			return errors.New("path has too many files")
		}
		if !info.Mode().IsRegular() {
			return errors.New("only regular files are allowed")
		}
		rel, e := filepath.Rel(root, p)
		if e != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
			return errors.New("path escapes archive root")
		}
		name := filepath.ToSlash(rel)
		if name == "" || strings.HasPrefix(name, "/") {
			return errors.New("invalid archive path")
		}
		if strings.ContainsAny(name, "\":<>|*?\r\n") {
			return errors.New("file path contains characters unsupported by GitHub artifacts")
		}
		if !includeHidden {
			for _, segment := range strings.Split(name, "/") {
				if strings.HasPrefix(segment, ".") {
					return nil // Match upload-artifact v4's default hidden-file exclusion.
				}
			}
		}
		files = append(files, sourceFile{full: p, archiveName: name, info: info})
		return nil
	}
	if !st.IsDir() {
		if err = add(resolved, st); err != nil {
			return nil, "", err
		}
	} else {
		err = filepath.WalkDir(resolved, func(p string, d os.DirEntry, walkErr error) error {
			if walkErr != nil {
				return walkErr
			}
			info, e := os.Lstat(p)
			if e != nil {
				return e
			}
			if info.Mode()&os.ModeSymlink != 0 {
				return errors.New("symlinks are not allowed")
			}
			if d.IsDir() {
				return nil
			}
			return add(p, info)
		})
		if err != nil {
			return nil, "", errors.New("could not read upload path")
		}
	}
	sort.Slice(files, func(i, j int) bool { return files[i].archiveName < files[j].archiveName })
	return files, root, nil
}

func createArchive(files []sourceFile, _ string) (string, int64, error) {
	f, err := os.CreateTemp("", "gha-artifact-*.zip")
	if err != nil {
		return "", 0, errors.New("could not create archive")
	}
	name := f.Name()
	zw := zip.NewWriter(f)
	for _, src := range files {
		h, e := zip.FileInfoHeader(src.info)
		if e != nil {
			f.Close()
			os.Remove(name)
			return "", 0, errors.New("could not create archive")
		}
		h.Name = src.archiveName
		h.Method = zip.Deflate
		w, e := zw.CreateHeader(h)
		if e != nil {
			f.Close()
			os.Remove(name)
			return "", 0, errors.New("could not create archive")
		}
		in, e := os.Open(src.full)
		if e != nil {
			f.Close()
			os.Remove(name)
			return "", 0, errors.New("upload file changed or disappeared")
		}
		opened, e := in.Stat()
		if e != nil || !os.SameFile(src.info, opened) {
			in.Close()
			f.Close()
			os.Remove(name)
			return "", 0, errors.New("upload file changed or disappeared")
		}
		_, e = io.Copy(w, in)
		in.Close()
		if e != nil {
			f.Close()
			os.Remove(name)
			return "", 0, errors.New("could not read upload file")
		}
	}
	if err = zw.Close(); err != nil {
		f.Close()
		os.Remove(name)
		return "", 0, errors.New("could not finalize archive")
	}
	if err = f.Close(); err != nil {
		os.Remove(name)
		return "", 0, errors.New("could not finalize archive")
	}
	st, err := os.Stat(name)
	if err != nil {
		os.Remove(name)
		return "", 0, errors.New("could not stat archive")
	}
	return name, st.Size(), nil
}

func (u *uploader) upload(ctx context.Context, name string, retention int, archive string, size int64) error {
	expires := any(nil)
	if retention > 0 {
		expires = u.now().UTC().Add(time.Duration(retention) * 24 * time.Hour).Format(time.RFC3339Nano)
	}
	create := map[string]any{"workflowRunBackendId": u.runBackend, "workflowJobRunBackendId": u.jobBackend, "name": name, "version": 4}
	if expires != nil {
		create["expiresAt"] = expires
	}
	var created createArtifactResponse
	if err := u.twirp(ctx, "CreateArtifact", create, &created); err != nil {
		return err
	}
	if !created.OK {
		return errors.New("artifact service rejected creation")
	}
	signedUploadURL := created.uploadURL()
	blobURL, err := url.Parse(signedUploadURL)
	if err != nil {
		return errors.New("artifact service returned a malformed storage URL")
	}
	if blobURL.Scheme != "https" || blobURL.Host == "" || blobURL.User != nil {
		return fmt.Errorf("artifact service returned an invalid storage URL (scheme=%q host-present=%t userinfo-present=%t length=%d)", blobURL.Scheme, blobURL.Host != "", blobURL.User != nil, len(signedUploadURL))
	}
	hash, err := u.uploadBlocks(ctx, blobURL, archive, size)
	if err != nil {
		return err
	}
	finalize := map[string]any{"workflowRunBackendId": u.runBackend, "workflowJobRunBackendId": u.jobBackend, "name": name, "size": strconv.FormatInt(size, 10), "hash": "sha256:" + hash}
	var done finalizeArtifactResponse
	if err := u.twirp(ctx, "FinalizeArtifact", finalize, &done); err != nil {
		return err
	}
	if !done.OK {
		return errors.New("artifact service rejected finalization")
	}
	return nil
}

func (u *uploader) twirp(ctx context.Context, method string, body any, result any) error {
	data, err := json.Marshal(body)
	if err != nil {
		return errors.New("could not encode artifact request")
	}
	endpoint := u.resultsURL + "/twirp/" + serviceName + "/" + method
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint, strings.NewReader(string(data)))
	if err != nil {
		return errors.New("could not create artifact request")
	}
	req.Header.Set("Authorization", "Bearer "+u.token)
	req.Header.Set("Content-Type", "application/json")
	resp, err := u.client.Do(req)
	if err != nil {
		return errors.New("artifact service request failed")
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return fmt.Errorf("artifact service request failed (HTTP %d)", resp.StatusCode)
	}
	if err = json.NewDecoder(io.LimitReader(resp.Body, 1<<20)).Decode(result); err != nil {
		return errors.New("artifact service response was invalid")
	}
	return nil
}

func (u *uploader) uploadBlocks(ctx context.Context, base *url.URL, archive string, size int64) (string, error) {
	f, err := os.Open(archive)
	if err != nil {
		return "", errors.New("archive is unavailable")
	}
	defer f.Close()
	digest := sha256.New()
	ids := []string{}
	buf := make([]byte, blockSize)
	for i := 0; ; i++ {
		n, readErr := io.ReadFull(f, buf)
		if readErr != nil && readErr != io.EOF && readErr != io.ErrUnexpectedEOF {
			return "", errors.New("could not read archive")
		}
		if n == 0 {
			break
		}
		_, _ = digest.Write(buf[:n])
		id := base64.StdEncoding.EncodeToString([]byte(fmt.Sprintf("%08d", i)))
		blockURL := withQuery(base, "comp", "block", "blockid", id)
		req, e := http.NewRequestWithContext(ctx, http.MethodPut, blockURL.String(), bytes.NewReader(buf[:n]))
		if e != nil {
			return "", errors.New("could not upload archive block")
		}
		req.ContentLength = int64(n)
		req.Header.Set("Content-Type", "application/octet-stream")
		req.Header.Set("x-ms-version", "2023-11-03")
		resp, e := u.client.Do(req)
		if e != nil {
			return "", errors.New("archive storage upload failed")
		}
		io.Copy(io.Discard, io.LimitReader(resp.Body, 4096))
		resp.Body.Close()
		if resp.StatusCode < 200 || resp.StatusCode >= 300 {
			return "", fmt.Errorf("archive storage upload failed (HTTP %d)", resp.StatusCode)
		}
		ids = append(ids, id)
		if readErr == io.ErrUnexpectedEOF || readErr == io.EOF {
			break
		}
	}
	if int64(len(ids)) == 0 && size != 0 {
		return "", errors.New("archive upload was incomplete")
	}
	var xmlBuf strings.Builder
	xmlBuf.WriteString("<?xml version=\"1.0\" encoding=\"utf-8\"?><BlockList>")
	for _, id := range ids {
		xmlBuf.WriteString("<Latest>")
		xmlBuf.WriteString(id)
		xmlBuf.WriteString("</Latest>")
	}
	xmlBuf.WriteString("</BlockList>")
	commitURL := withQuery(base, "comp", "blocklist", "", "")
	req, e := http.NewRequestWithContext(ctx, http.MethodPut, commitURL.String(), strings.NewReader(xmlBuf.String()))
	if e != nil {
		return "", errors.New("could not commit archive")
	}
	req.Header.Set("Content-Type", "application/xml")
	req.Header.Set("x-ms-version", "2023-11-03")
	req.Header.Set("x-ms-blob-content-type", "zip")
	resp, e := u.client.Do(req)
	if e != nil {
		return "", errors.New("archive storage commit failed")
	}
	io.Copy(io.Discard, io.LimitReader(resp.Body, 4096))
	resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return "", fmt.Errorf("archive storage commit failed (HTTP %d)", resp.StatusCode)
	}
	return hex.EncodeToString(digest.Sum(nil)), nil
}

func withQuery(base *url.URL, k1, v1, k2, v2 string) *url.URL {
	u := *base
	q := u.Query()
	if k2 != "" {
		q.Set(k2, v2)
	}
	q.Set(k1, v1)
	u.RawQuery = q.Encode()
	return &u
}
