# Team MCP browser authorization

Team MCP supports an optional authorization-code browser entry. An agent's
MCP client discovers the service, registers a callback and opens the AWR consent
page. The user enters their personal credential there and confirms the project.
The client receives a short-lived, project-bound token instead of the personal
credential. Native-client compatibility requires verification with that client;
protocol tests alone do not establish it.

The adapter retains the existing Team identity and permissions. It does not
create an account, project membership, grant, claim or task delegation. The HTTP
entry verifies current project access before approving consent, then rechecks
the underlying credential and authorization in each MCP operation.

## Policy

- Public clients register exact HTTPS or HTTP loopback redirect URIs. Wildcards,
  userinfo, fragments, ambiguous URL normalization and preexisting OAuth result
  parameters are rejected. Registration performs no remote fetch.
- Authorization requires S256 PKCE, a registered client/callback and one exact
  operator-configured HTTPS resource. Client state is returned unchanged when
  supplied; browser consent has its own separate binding.
- Consent transactions expire after 10 minutes. Approval requires the original
  browser binding and consumes the transaction atomically.
- Codes expire after 2 minutes. A well-formed exchange consumes the code even
  when client, callback, resource or verifier does not match. Concurrent exchange
  can issue at most one access token.
- Opaque access tokens last at most 1 hour and resolve for one resource only.
  They neither contain nor return the original personal credential. Credential
  expiry, revocation and permission changes must still be enforced by Team.

## Storage and bounds

All registrations, consent, codes and tokens are process-local. Restarting the
server invalidates them. No refresh token or durable sign-in is provided. The
adapter hashes browser bindings, codes and access tokens before storing their
lookup keys; underlying personal credentials stay in memory only.

Registrations expire after 24 hours. There are at most 1,024 registrations,
1,024 consent transactions, 1,024 codes and 4,096 access tokens. Expired entries
are pruned before mutations; reaching a live capacity returns a generic
temporary-unavailability error. OAuth bodies and query strings are limited to
16 KiB and share the service's 64-request admission limit and 30-second timeout.

## Enable the browser entry

OAuth is disabled unless the operator adds an issuer to the service configuration:

```toml
version = 1
listen = "127.0.0.1:9910"
allowed_hosts = ["team.example.org"]

[oauth]
issuer = "https://team.example.org"

[[projects]]
key = "portal"
tenant_id = "team"
project_id = "customer-portal"
```

The issuer must be a canonical HTTPS origin without a trailing slash, explicitly
listed in `allowed_hosts`. It is never inferred from `Host` or forwarded headers.
Terminate TLS at the trusted proxy, preserve this Host, and route the following
paths to the same server process:

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/.well-known/oauth-authorization-server` | Authorization metadata |
| GET | `/.well-known/oauth-protected-resource/v1/projects/{key}/mcp` | Exact resource metadata |
| POST | `/oauth/register` | Public-client registration; `none` authentication |
| GET | `/oauth/authorize` | `response_type=code`, S256 PKCE and exact `resource` |
| POST | `/oauth/consent` | Browser-bound approval or cancellation |
| POST | `/oauth/token` | URL-encoded authorization-code exchange |
| GET/POST/DELETE | `/v1/projects/{key}/mcp` | Existing MCP transport |

Missing MCP authentication returns `401` with a `WWW-Authenticate` resource
metadata URL when OAuth is enabled. Expired or invalid OAuth tokens also return
`401`. OAuth-disabled services retain the previous `403` behavior. Existing
static bearer authentication remains available. An OAuth token is accepted only
at its exact MCP resource, never as a classic HTTP or Inspector credential.

The supported scope is `awr.project`. Authorization and token exchange require
the exact project resource URL. Registration may request refresh along with the
authorization-code grant, but the response advertises only authorization code;
no refresh token is issued. An expired connection or server restart requires a
new connection and consent, including re-registration if necessary.

## Consent and proxy handling

Consent requires the original Secure/HttpOnly/SameSite browser cookie, an exact
issuer Origin and the bound transaction. Duplicate form/query parameters and
ambiguous security headers are rejected. Approval checks current project access
before issuing a code; cancellation consumes the transaction and returns
`access_denied` with the client's original state. Invalid credentials are not
echoed into the retry page or errors.

Client names and callbacks are escaped and explicitly labeled as self-registered.
The page uses no scripts or external assets. Responses set no-store, frame
protection and a restrictive CSP. Consent forms use `Referrer-Policy: same-origin`
so browsers preserve the issuer Origin on the form POST without sending a
referrer to another origin. Completion pages and other OAuth responses use
`no-referrer`. Opaque (`null`), missing and foreign Origins remain rejected.
The form posts only to AWR and its CSP keeps `form-action 'self'`.

Successful approval and cancellation return a `200` HTML completion page that
automatically navigates to the exact registered callback, with the code or error
and original state. A fallback link targets the same URL. The cookie is cleared,
and the page contains no credential, scripts or external assets. This ends the
form submission before navigating: browsers can apply `form-action` to an HTTP
redirect's entire chain, including a client's later redirect to another origin.
An HTTP client testing this endpoint must read the completion page rather than
expect a `303` Location header; PKCE, token exchange and callback binding are
unchanged. Starting a second authorization in the same browser replaces the
first consent cookie; restart the earlier connection rather than reusing its page.

Do not log consent bodies, token responses, cookies or Authorization headers.
At the proxy, suppress or redact OAuth access-log query strings: authorization
URLs contain state and PKCE material, and callbacks contain single-use codes.
Do not include personal credentials in agent chat, URLs or MCP configuration.
Browser consent does not change project permissions or task delegation.

HTTP, SDK and policy regressions are distinct from actual browser/native-client
verification and from business acceptance. Connecting a client alone is not
evidence that a collaborative development scenario is complete.
