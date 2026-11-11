# Security policy

Wasl handles file contents, Google account tokens and an encrypted vault format, so we want security reports early
and in private.

## Reporting a vulnerability

Please report it **privately**. Do not open a public issue or pull request.

Use GitHub private vulnerability reporting: open the repository's **Security** tab, choose **Report a
vulnerability** (https://github.com/Ahmad-AlModhayan/wasl/security/advisories/new). If you cannot use GitHub, write
to hello@kahflabs.com with "security" in the subject.

Include what you found, the version or commit, and steps to reproduce. Reports about the vault format
(`engine/crates/wasl-crypto`), the device-pairing and IPC authentication (`wasld`), OAuth token storage, and
anything that could delete or overwrite a user's files are the most important.

## What to expect

We answer within a few days, confirm the problem, tell you when a fix is planned, and credit you in the advisory
if you want. Please give us time to ship a fix before you publish details. We work embargoed fixes in a private
fork and publish them with the advisory.

## Supported versions

| Version | Supported |
|---|---|
| Latest release | yes |
| Earlier releases | no |
| `main` | fixes land here first |

## What is and is not a secret in this repository

The repository holds no signing keys, refresh tokens or service accounts. The Google OAuth client for the desktop
app is passed in at build time (`WASL_GOOGLE_CLIENT_ID`, `WASL_GOOGLE_CLIENT_SECRET`, see
[docs/SETUP.md](docs/SETUP.md)). Keys and vectors under `*/tests/` are made-up test values. If you find a real
secret in the tree or its history, report it as above.

The security review notes are in [docs/SECURITY.md](docs/SECURITY.md).
