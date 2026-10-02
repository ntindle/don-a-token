# SIWC implementation notes

Distilled from the Sign in with ChatGPT token-sharing docs for open-source
clients. Sources: [overview](https://developers.openai.com/siwc/token-sharing-open-source),
[full export](https://developers.openai.com/siwc/llms-full.txt),
[index](https://developers.openai.com/siwc/llms.txt).
Implemented in `crates/core/src/siwc.rs` + `credentials.rs`.

## Endpoints

- Authorize: `GET https://auth.openai.com/api/accounts/authorize`
- Token: `POST https://auth.openai.com/api/accounts/oauth/token` (form-encoded)
- OIDC discovery: `https://auth.openai.com/.well-known/openid-configuration`
  (revocation endpoint lives here)
- Models: `GET https://api.openai.com/v1/models` (filter `visibility == "list"`)
- Inference: `POST https://api.openai.com/v1/responses`
  (`store: false`, `stream: true`, wait for `response.completed`)

## Authorize parameters

First registration: `client_id=dynamic_agent_client` +
`agent_name_hint=don-a-token` (consistent everywhere, incl. Codex
`clientInfo.name`) + required `ext_agent_host_id` (stable per host,
`urn:uuid:…` for now, JWK-thumbprint upgrade later).

Reauthorization: saved issued `client_id`, same host id, fresh
state/nonce/PKCE, retained `id_token_hint` (+ optional `login_hint`);
omit `agent_name_hint`.

Fixed: `response_type=code`, `redirect_uri=http://127.0.0.1:<port>/auth/callback`
(only the port may vary; never `localhost`),
`scope=openid profile email offline_access resource.invoke chatgpt.tokens.use.direct`,
`resource=https://api.openai.com/v1`, PKCE `S256`.

## Callback + exchange

New-registration callback returns `code`, `state`, issued `client_id`
(like `oaiapp_…`), sometimes `scope`. Reauth may omit `client_id` —
retain the pending id; reject if a *different* id arrives. Never save
`dynamic_agent_client` as the issued id. `error=access_denied` stops the
attempt (plan usage can be enabled later from settings).
Exchange needs no client secret. `invalid_grant` → fresh auth with the
retained client id.

## Validation + storage

Verify the ID token signature (JWKS), `iss`, `aud` = issued client id,
`exp`, and the attempt `nonce`; `sub` is the account identity. Then check
granted scopes for `chatgpt.tokens.use.direct` — a valid ID token alone
does not authorize plan usage.

Store one record per issued client id
(`email, issuer, subject, client_id, ext_agent_host_id, id_token,
access_token, refresh_token, token_type, expires_in, scopes, saved_at`),
atomically, owner-only. Keep the ID token for `id_token_hint`.

## Lifetimes

Access tokens: 1 hour. Refresh tokens: 30 days, rolling replacement on
each successful refresh. Refresh with the 5-minute skew window, then
restart app-server with the new `ACCESS_TOKEN` and `thread/resume`.

## Errors that matter

- Missing `chatgpt.tokens.use.direct` → sign-in stays, plan usage marked
  disabled; offer enable-later or another billing path.
- Direct admission before stream: `401` (identity/permission),
  `403` (policy/region), `503` (routing unavailable, back off).
- Structured codes: `subscription_sharing_user_not_eligible` (403, don't
  retry same request), `subscription_sharing_usage_limit_exceeded` /
  `subscription_sharing_usage_unavailable` (can arrive mid-stream as
  `response.failed` → halt donations, primary action **Manage usage**).
- Preview constraints: no `temperature`/`top_p`/`max_output_tokens`/
  `metadata`/`user`/etc.; no `previous_response_id` over HTTP (send
  history in `input`); no image gen / file search / Code Interpreter /
  computer use / hosted MCP; app-server sets `store:false, stream:true`
  itself — never emit Responses `tool_search`.

## UI rules (must-follow)

- Button label **Continue with ChatGPT** with approved branding.
- First-sign-in modal **You're using your ChatGPT plan** + **Got it**, once.
- Settings card for plan use; **Manage usage** link
  (`https://chatgpt.com/settings/usage`) near usage + plan indicator;
  **Using ChatGPT plan** near composer/model selector; usage-limit modal
  with Manage usage primary.

## Multi-machine credentials

Each machine gets its own host id. OAuth completes on the machine with the
browser; the credential file transfers over SSH to the headless host's
documented path without overwriting its host id; that host then owns
refreshes. See [TESTING](TESTING.md).
