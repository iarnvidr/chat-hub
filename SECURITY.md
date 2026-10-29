# Security policy

Huginn is planned to run locally and handle sensitive conversation content.
The current scaffold is not a production-ready security boundary; do not use it
to capture sensitive data.

Report vulnerabilities privately to the repository maintainers through GitHub
Security Advisories (or the repository's private vulnerability reporting
feature). Do not include secrets or personal conversation data in public
issues. Include affected revision, reproduction steps, and impact where safe.

The intended design includes local token-authenticated API access and
selective TLS interception. Until implemented and reviewed, these are plans,
not guarantees. Users must not install a local CA or route provider traffic
through an unreviewed build.
