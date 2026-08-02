# Security

## Reporting a vulnerability

Use GitHub private vulnerability reporting for security issues. Do not include exploit details, credentials, or production data in a public issue.

Include the affected revision, expected behavior, observed behavior, and a minimal reproduction when possible.

## Deployment assumptions

SCG does not implement user identity or TLS termination. Keep the node on a private interface or place it behind an authenticated TLS proxy. The supplied Compose configuration binds the API to `127.0.0.1`.

Each realm must keep its own process, database volume, and deployment credentials. Do not mount a live database into a lab node.
