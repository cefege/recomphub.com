# RecompHub

RecompHub (recomphub.com) is a searchable database of console and PC game recompilations and native ports. Each game has a page listing its known ports, where they run, their authors, AI usage, GitHub stars, release status and gameplay videos.

The site is a Rust workspace:

- `crates/shared`: serde DTOs, enums, slug helpers and SEO text builders (compiles to wasm).
- `crates/db`: sqlx Postgres pool, migrations, queries and submission logic.
- `crates/ingest`: IGDB, GitHub, YouTube, Reddit and seed-import clients, AI and platform detection, media pipeline.
- `crates/web`: the Dioxus 0.7 fullstack app (binary `server` when built with `dx`).
- `crates/worker`: the `worker` CLI that runs sync and import jobs.

## Local development

```sh
brew services start postgresql@18
createdb recomphub
createdb recomphub_test
cp .env.example .env        # then fill in the keys listed below
dx serve --package recomphub
```

Run the worker commands from the repository root:

```sh
cargo run -p recomphub-worker -- catalog-sync
cargo run -p recomphub-worker -- import-recompendium
cargo run -p recomphub-worker -- import-portsdr
cargo run -p recomphub-worker -- github-sync
cargo run -p recomphub-worker -- media-sync
cargo run -p recomphub-worker -- youtube-sync
cargo run -p recomphub-worker -- discover
cargo run -p recomphub-worker -- port-videos
cargo run -p recomphub-worker -- describe-ports --dry-run
cargo run -p recomphub-worker -- describe-games --dry-run --game need-for-speed-most-wanted
cargo run -p recomphub-worker -- stats-snapshot
cargo run -p recomphub-worker -- sales-sync
cargo run -p recomphub-worker --bin discord-setup -- --dry-run --prune --server-icon
cargo run -p recomphub-worker --bin discord-setup -- --prune --server-icon
```
`discord-setup` maintains five community channels: `#welcome`, `#announcements`, `#general`, `#showcase`, and `#website-bugs`. Their channel topics supply onboarding and posting guidance. `--server-icon` applies the bundled RecompHub logo; `--prune` deletes every other channel and category, so use it only for an intentional clean cutover.


Tests:

```sh
DATABASE_URL=postgres://localhost/recomphub_test cargo test --workspace --features recomphub/server
```

## Environment variables

Copy `.env.example` to `.env` for local development. In production, these come from the `recomphub-secrets` Secret.

| Variable | Default / format |
|---|---|
| `DATABASE_URL` | required |
| `SITE_URL` | `http://localhost:8080` |
| `MEDIA_DIR` | `./data/media` |
| `GITHUB_TOKEN` | |
| `TWITCH_CLIENT_ID` | |
| `TWITCH_CLIENT_SECRET` | |
| `YOUTUBE_API_KEY` | |
| `SECRETS_KEY` | 32 bytes, base64 (`openssl rand -base64 32`); encrypts AI provider credentials at rest. Web and workers need the same value |
| `YTDLP_BIN` | `yt-dlp` |
| `YTDLP_COOKIES` | optional path to a Netscape cookies.txt; copied to a temp file, never logged |
| `GOOGLE_OAUTH_CLIENT_ID` | Google OAuth client ID (Web application); the only sign-in method. `/auth/google` returns 503 without it. Redirect URI: `${SITE_URL}/auth/google/callback` |
| `GOOGLE_OAUTH_CLIENT_SECRET` | Google OAuth client secret |
| `ADMIN_EMAILS` | comma-separated; a Google sign-in whose verified email is listed gets the admin role |
| `RUST_LOG` | |
| `DISCORD_BOT_TOKEN` | Discord bot token for the one-shot `discord-setup` command; grants the bot API access to configure the RecompHub server |
| `IP` and `PORT` | set by the Dockerfile; dx reads them |

## Container and deployment

- `Dockerfile` builds the site with `dx bundle --web --release --package recomphub` and the worker with `cargo build --release -p recomphub-worker`. The runtime image puts the bundle in `/usr/local/app/` (entrypoint `server`) and the worker at `/usr/local/app/worker`. If the bundle path printed by `dx bundle` differs from `target/dx/recomphub/release/web/`, update the `COPY` line in the Dockerfile.
- CI (`.github/workflows/ci.yml`) runs fmt, clippy and tests against a `postgres:17-alpine` service.
- `port-videos` (CronJob `port-videos`, daily 04:30 UTC) finds gameplay videos for each port with yt-dlp, not the YouTube Data API. Sources: YouTube links in each port's README (IDs recorded by `github-sync`) and relevance-filtered `ytsearch` results per port. Searches are throttled and capped per run. The optional Secret `recomphub-ytdlp-cookies` (key `cookies.txt`) is mounted read-only at `/secrets/ytdlp`; it is only needed when YouTube blocks anonymous requests. Upgrade the pinned `YTDLP_VERSION` in the Dockerfile when YouTube breaks extraction.
- `github-sync`'s weekly AI/author scan also reads each port's README for screenshots: markdown and `<img>` images, filtered of badges, shields, stat cards and logos (`recomphub_ingest::readme_images`), resolved to absolute URLs and capped at 6 per port. They are attached as already-approved `port_media` rows, trusted the same way README-linked videos are, and show in the port's gallery ahead of the game's IGDB screenshots. Member-uploaded screenshots still go through moderation separately.
- `sales-sync` (CronJob `sales-sync`, Mondays 04:00 UTC) matches Wikipedia's best-selling game lists (per platform, overall, and per year) to catalog games. Each game's `sales_score` is `round(300 * log2(1 + best units in millions))` across its matches, so a million-seller sits near the 90th percentile of IGDB popularity. `games.importance` is the generated column `popularity + sales_score`, and all catalog ordering uses it. A list that fails to fetch or parse keeps its previous matches and fails the run. `cargo run -p recomphub-worker -- sales-sync --dry-run` prints the parsed rows without writing.
- `describe-ports` (CronJob `describe-ports`, daily 06:30 UTC) writes a short description of each port from the repository text that `github-sync` stores: README, topics and latest release notes. It uses the AI providers configured at `/admin` → AI providers (Anthropic or OpenAI API keys, or Claude / ChatGPT subscription logins), tried in the order shown there with fallback to the next on any failure, and accepts only output that matches the schema and names no ROM download site. Each run makes at most 200 calls, least recently attempted first, and regenerates a description only when its stored text or port facts change. With no enabled provider, or without `SECRETS_KEY`, the job logs and exits. `describe-ports --dry-run` prints the next request and calls nothing.
- `describe-games` (CronJob `describe-games`, daily 06:45 UTC) writes each game page's FAQ from the facts the database holds: original platforms, ports, decompilations, release, maintenance level, authors and requirements. It runs only for games with a port or decompilation, uses the same AI providers as `describe-ports`, and stores the first answer that passes the checks (3 to 7 items, no yes/no openers, no names or numbers missing from the facts, no URLs or ROM-source wording). Otherwise the page keeps the built-in FAQ. Each run makes at most 60 calls, and a game whose last attempt failed on the same facts is retried after 3 days. `describe-games --dry-run --game <slug>` prints the prompt, facts and schema and calls nothing.
- `stats-snapshot` (CronJob `stats-snapshot`, daily 03:30 UTC) stores today's platform parity counters in `stats_daily` (source `snapshot`, one row per day, rewritten if the job runs again that day). It then rebuilds every earlier day from each project's GitHub repository creation date, or the day it was added when GitHub has no date (source `backfill`, replaced on each run). The `/stats` page reads these rows.
- `.github/workflows/image.yml` builds and pushes `ghcr.io/cefege/recomphub.com:sha-<12>` on each push to `main`, then pins the tag in `deploy/production/kustomization.yaml`.
- Flux applies `deploy/production/` on the `ovh1` k3s cluster through `deploy/flux/bootstrap.yaml`.

## Operator steps

These need your Cloudflare (DNS and tunnel), Google Cloud, cluster and GitHub access.

1. Create the public repo `cefege/recomphub.com` and push. The GHCR package `ghcr.io/cefege/recomphub.com` inherits the repo's public visibility, so the cluster pulls it without a pull secret.
2. Create the tunnel and wire DNS:
   - `cloudflared tunnel create recomphub`
   - `kubectl -n recomphub create secret generic cloudflared-credentials --from-file=credentials.json=<path to credentials.json>`
   - `cloudflared tunnel route dns recomphub recomphub.com`
   - `cloudflared tunnel route dns recomphub www.recomphub.com`
   - Put the tunnel ID in `deploy/production/cloudflared.yaml` (currently tunnel `recomphub`, `152ceebc-cbe8-442d-9b03-54bf2f37e4e0`).
3. Create the Google sign-in client in Google Cloud Console: **APIs & Services → OAuth consent screen** (External, app name RecompHub, scopes `openid`, `email`, `profile`, then **Publish app** so any Google account can sign in), then **Credentials → Create credentials → OAuth client ID → Web application** with authorized JavaScript origin `https://recomphub.com` and authorized redirect URI `https://recomphub.com/auth/google/callback`. Note the client ID and secret for the secret below.
   Everyone, the admin included, signs in with Google; "Sign in" goes straight to `/auth/google`. The admin role goes to a Google account whose verified email is in `ADMIN_EMAILS`.
4. Create the application secret:

   ```sh
   kubectl -n recomphub create secret generic recomphub-secrets \
     --from-literal=POSTGRES_PASSWORD="$(openssl rand -hex 24)" \
     --from-literal=DATABASE_URL="postgres://recomphub:<same password>@postgres:5432/recomphub" \
     --from-literal=GOOGLE_OAUTH_CLIENT_ID=... \
     --from-literal=GOOGLE_OAUTH_CLIENT_SECRET=... \
     --from-literal=GITHUB_TOKEN=... \
     --from-literal=TWITCH_CLIENT_ID=... \
     --from-literal=TWITCH_CLIENT_SECRET=... \
     --from-literal=YOUTUBE_API_KEY=... \
     --from-literal=SECRETS_KEY="$(openssl rand -base64 32)" \
     --from-literal=ADMIN_EMAILS=mateiasmihaiandrei@gmail.com
   ```

   The namespace is created by Flux, so run `kubectl create namespace recomphub` first if you create the secret before the first sync.
5. On `ovh1`, apply the Flux bootstrap: `kubectl apply -f deploy/flux/bootstrap.yaml`. Then run `kubectl -n recomphub create job --from=cronjob/postgres-backup postgres-backup-init` once: the `local-path` backup volume binds only when a pod first uses it, and Flux reports the Kustomization unhealthy until it is bound.
6. Run each worker once, in this order, with `kubectl -n recomphub create job --from=cronjob/<name> <name>-init`: `catalog-sync`, `import-recompendium`, `import-portsdr`, `github-sync`, `sales-sync`, `media-sync`.
7. Check `curl -s https://recomphub.com/healthz`; it should return `ok`.

## Attribution

Seed data: Recompendium (MIT).
