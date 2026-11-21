"""Pure policy tests for the headless ChatGPT browser; synthetic URLs only."""
import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "server-browser"))
from policy import conversation_path, navigation_url, request_allowed, signed_out_url  # noqa: E402


class PolicyTests(unittest.TestCase):
    def test_navigation_is_https_chatgpt_only(self):
        self.assertEqual(navigation_url("https://chatgpt.com/c/abc-123#frag"), "https://chatgpt.com/c/abc-123")
        for url in ["http://chatgpt.com/", "file:///etc/passwd", "https://chatgpt.com.evil.test/",
                    "https://chatgpt.com:8443/", "https://u:p@chatgpt.com/", "https://example.test/", "javascript:1"]:
            with self.assertRaises(ValueError, msg=url):
                navigation_url(url)
            self.assertFalse(request_allowed(url, "document", True), url)

    def test_subresources_and_media(self):
        self.assertTrue(request_allowed("https://chatgpt.com/backend-api/me", "fetch", False))
        self.assertTrue(request_allowed("https://cdn.oaistatic.com/a.js", "script", False))
        self.assertTrue(request_allowed("https://challenges.cloudflare.com/x", "script", False))
        self.assertFalse(request_allowed("https://cloudflare.com/x", "script", False))
        self.assertFalse(request_allowed("https://tracker.example.test/p.gif", "image", False))
        self.assertFalse(request_allowed("https://files.oaiusercontent.com/a.mp4", "fetch", False))
        self.assertFalse(request_allowed("https://chatgpt.com/a", "media", False))
        self.assertFalse(request_allowed("https://cdn.oaistatic.com/a.js", "script", True))

    def test_sign_in_detection_and_conversation_links(self):
        self.assertTrue(signed_out_url("https://auth.openai.com/log-in"))
        self.assertTrue(signed_out_url("https://chatgpt.com/auth/login"))
        self.assertFalse(signed_out_url("https://chatgpt.com/c/abc"))
        self.assertEqual(conversation_path("/c/123e4567-e89b-12d3-a456-426614174000"), "/c/123e4567-e89b-12d3-a456-426614174000")
        for bad in ["/c/", "/c/a/b", "https://evil.test/c/../x", "/g/g-p-1", "", None]:
            self.assertIsNone(conversation_path(bad), bad)


if __name__ == "__main__":
    unittest.main()
