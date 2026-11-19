# MyTune key server

A Cloudflare Worker with a D1 database. It hands out licence keys, binds each key to one PC and signs a token
that MyTune checks offline for 7 days. You manage keys with the key manager app (`mytune-keys`).

There is no rate limit. Keys have 100 random bits, so guessing one is not practical.

## Set it up (once)

All commands run in PowerShell. Steps 5 onwards run inside this `keyserver` folder.

1. Create a free Cloudflare account at https://dash.cloudflare.com/sign-up and confirm your email.
2. Install Node.js (LTS) from https://nodejs.org if `node --version` does not work, then install Wrangler:
   `npm i -g wrangler`
3. Sign in: `wrangler login` (a browser window opens; allow access).
4. Go to this folder: `cd C:\Users\domin\Documents\MyTune\mytune-rs\keyserver`
5. Create the database: `wrangler d1 create mytune-keys`
   It prints a block with `database_id = "..."`. Copy that id into `wrangler.toml` in place of
   `PUT-THE-DATABASE-ID-HERE`.
6. Create the table: `wrangler d1 execute mytune-keys --remote --file=schema.sql`
7. Make the signing key pair. From the workspace folder (`..`):
   `.\build.ps1 run -p mytune-keys -- keygen`
   It prints two lines of base64: `SIGNING_KEY` (private, never share it) and `PUBLIC_KEY`.
   Keep the window open for steps 8 and 11.
8. Back in `keyserver`: `wrangler secret put SIGNING_KEY` and paste the private line when asked.
9. Make up a long admin password (40+ random characters, e.g. from a password manager), then
   `wrangler secret put ADMIN_SECRET` and paste it.
10. Publish: `wrangler deploy`. It prints the worker URL, like `https://mytune-keys.<your-name>.workers.dev`.
11. In `crates/mytune-core/src/license.rs` set `SERVER_URL` to that URL and `PUBLIC_KEY` to the public line
    from step 7. Then rebuild MyTune.
12. Start the key manager (`.\build.ps1 run -p mytune-keys`), enter the worker URL and the admin password from
    step 9. They are saved in `%AppData%\MyTuneKeys\config.json`.

## Day to day

- New key: "New key", type who it is for. The key is copied to the clipboard; send it to your friend.
- Friend has a new PC: "Unbind", then they enter the key again on the new PC.
- Take a key away: "Revoke" (MyTune stops working within 7 days, when its token runs out). "Allow again" undoes it.
- Devices not seen for 30+ days are shown in orange: likely unused, unbind or delete them.
- After changing `worker.js`, run `wrangler deploy` again. If you lose `SIGNING_KEY`, make a new pair and
  rebuild MyTune with the new public key; old tokens then stop working.

## API

- `POST /activate` `{"key","device","name"}` -> `{"token"}` or 403 `{"error":"invalid"|"revoked"|"bound"}`
- Admin (header `Authorization: Bearer <ADMIN_SECRET>`): `GET /admin/keys`, `POST /admin/keys {"note"}`,
  `POST /admin/revoke {"key","revoked"}`, `POST /admin/unbind {"key"}`, `POST /admin/delete {"key"}`
