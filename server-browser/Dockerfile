# Headless ChatGPT capture browser. Runs beside chat-hub-web in the same container
# or pod; see docs/headless-capture.md. Chromium keeps its sandbox (no --no-sandbox):
# run with chromium-seccomp.json and unprivileged user namespaces available.
FROM python:3.12-slim-bookworm@sha256:e697c81897302a4c4333bade000d98038fbb21eb064f8ee1fd42d02ebcd9d337
ENV PYTHONUNBUFFERED=1 PYTHONDONTWRITEBYTECODE=1 \
    PLAYWRIGHT_BROWSERS_PATH=/opt/chromium \
    CHAT_HUB_BROWSER_ROOT=/var/lib/chat-hub-browser HOME=/var/lib/chat-hub-browser
COPY requirements.txt /tmp/requirements.txt
# libnss3-tools provides certutil, used to trust the capture CA inside the profile only.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libnss3-tools \
    && pip install --no-cache-dir -r /tmp/requirements.txt \
    && patchright install --with-deps chromium \
    && rm -rf /var/lib/apt/lists/* /tmp/requirements.txt \
    && useradd --uid 10001 --create-home --home-dir /var/lib/chat-hub-browser chat-hub-browser
COPY *.py /opt/chat-hub-browser/
USER 10001:10001
WORKDIR /opt/chat-hub-browser
HEALTHCHECK --interval=30s --timeout=5s --start-period=60s --retries=4 CMD ["python3", "/opt/chat-hub-browser/health.py"]
ENTRYPOINT ["python3", "/opt/chat-hub-browser/main.py"]
