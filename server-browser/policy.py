"""Request boundary for the headless ChatGPT browser. Pure functions only.

The capture proxy intercepts TLS for exactly `chatgpt.com`; every other allowed
host is tunnelled untouched. Keep the lists small: each entry is a host ChatGPT
needs to render or to pass its Cloudflare challenge.
"""
from __future__ import annotations
from urllib.parse import urlsplit, urlunsplit

START_URL = "https://chatgpt.com/"
# Pages the browser may navigate to. auth.openai.com is only where a signed-out
# session is redirected; the collector detects it and never signs in.
NAV_HOSTS = frozenset({"chatgpt.com", "auth.openai.com"})
# Sub-resource hosts (exact host or any subdomain).
RESOURCE_DOMAINS = ("chatgpt.com", "openai.com", "oaistatic.com", "oaiusercontent.com")
RESOURCE_HOSTS = frozenset({"challenges.cloudflare.com"})
BLOCKED_RESOURCE_TYPES = frozenset({"media"})
MEDIA_SUFFIXES = (".mp4", ".m4v", ".m3u8", ".mp3", ".wav", ".webm")


def parsed_https(value: str):
    if not isinstance(value, str) or len(value) > 4096 or any(ord(c) < 32 for c in value):
        raise ValueError("Invalid URL")
    parts = urlsplit(value)
    if parts.scheme != "https" or parts.username or parts.password or parts.port not in (None, 443):
        raise ValueError("Only HTTPS URLs are allowed")
    return parts


def navigation_url(value: str) -> str:
    parts = parsed_https(value)
    if parts.hostname not in NAV_HOSTS:
        raise ValueError("Only ChatGPT pages can be opened")
    return urlunsplit(("https", parts.netloc, parts.path or "/", parts.query, ""))


def request_allowed(value: str, resource_type: str, navigation: bool) -> bool:
    try:
        parts = parsed_https(value)
    except ValueError:
        return False
    host = parts.hostname or ""
    if navigation:
        return host in NAV_HOSTS
    if resource_type in BLOCKED_RESOURCE_TYPES or parts.path.lower().endswith(MEDIA_SUFFIXES):
        return False
    return host in RESOURCE_HOSTS or any(host == d or host.endswith("." + d) for d in RESOURCE_DOMAINS)


def signed_out_url(value: str) -> bool:
    """True when the browser landed on a sign-in surface rather than the app."""
    try:
        parts = parsed_https(value)
    except ValueError:
        return False
    return parts.hostname == "auth.openai.com" or (
        parts.hostname == "chatgpt.com" and parts.path.startswith(("/auth/", "/login")))


def conversation_path(href: str) -> str | None:
    """Return `/c/<id>` for a sidebar link, or None. IDs never leave this module's callers."""
    path = urlsplit(href).path if isinstance(href, str) and len(href) < 512 else ""
    parts = path.split("/")
    if len(parts) == 3 and parts[:2] == ["", "c"] and parts[2].replace("-", "").isalnum():
        return path
    return None
