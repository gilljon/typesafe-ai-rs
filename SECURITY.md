# Security policy

## Reporting a vulnerability

Use GitHub's [private vulnerability reporting](https://github.com/gilljon/typesafe-ai-rs/security/advisories/new)
for security issues in this SDK. Include the affected version, impact, and a
minimal reproduction with credentials and private data removed. Please keep
undisclosed vulnerabilities out of public issues and pull requests.

Issues with the hosted TypeSafe service should be reported directly to TypeSafe.

## Supported versions

Security fixes target the latest published release. Older releases are not
maintained separately. This project is maintained independently; there is no
guaranteed response time.

## Credentials and logging

Pass API keys through `TYPESAFE_API_KEY` or the client builder. Never commit keys
or include them in issues. The SDK redacts credential headers from its own logs
and debug output, but debug-level logging includes request and response bodies.
Those bodies can contain private application data. Applications control their
logger, custom HTTP clients, and any direct access to raw response data.
