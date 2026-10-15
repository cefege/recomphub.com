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
```

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
| `GITHUB_OAUTH_CLIENT_ID` | |
| `GITHUB_OAUTH_CLIENT_SECRET` | |
| `GITHUB_TOKEN` | |
| `TWITCH_CLIENT_ID` | |
| `TWITCH_CLIENT_SECRET` | |
| `YOUTUBE_API_KEY` | |
| `ADMIN_GITHUB_LOGINS` | comma-separated |
| `RUST_LOG` | |
| `IP` and `PORT` | set by the Dockerfile; dx reads them |

## Container and deployment

- `Dockerfile` builds the site with `dx bundle --web --release --package recomphub` and the worker with `cargo build --release -p recomphub-worker`. The runtime image puts the bundle in `/usr/local/app/` (entrypoint `server`) and the worker at `/usr/local/app/worker`. If the bundle path printed by `dx bundle` differs from `target/dx/recomphub/release/web/`, update the `COPY` line in the Dockerfile.
- CI (`.github/workflows/ci.yml`) runs fmt, clippy and tests against a `postgres:17-alpine` service.
- `.github/workflows/image.yml` builds and pushes `ghcr.io/cefege/recomphub.com:sha-<12>` on each push to `main`, then pins the tag in `deploy/production/kustomization.yaml`.
- Flux applies `deploy/production/` on the `ovh1` k3s cluster through `deploy/flux/bootstrap.yaml`.

## Operator steps

These need your Cloudflare, cluster and GitHub access.

1. Create the public repo `cefege/recomphub.com` and push. After the first image push, make the GHCR package `ghcr.io/cefege/recomphub.com` public.
2. Create the tunnel and wire DNS:
   - `cloudflared tunnel create recomphub`
   - `kubectl -n recomphub create secret generic cloudflared-credentials --from-file=credentials.json=<path to credentials.json>`
   - `cloudflared tunnel route dns recomphub recomphub.com`
   - `cloudflared tunnel route dns recomphub www.recomphub.com`
   - Put the tunnel ID in `deploy/production/cloudflared.yaml` (currently tunnel `recomphub`, `97edbe63-be7a-4e20-98ab-34adc117fa5e`).
3. Create a GitHub OAuth app with the callback `https://recomphub.com/auth/github/callback`.
4. Create the application secret:

   ```sh
   kubectl -n recomphub create secret generic recomphub-secrets \
     --from-literal=POSTGRES_PASSWORD="$(openssl rand -hex 24)" \
     --from-literal=DATABASE_URL="postgres://recomphub:<same password>@postgres:5432/recomphub" \
     --from-literal=GITHUB_OAUTH_CLIENT_ID=... \
     --from-literal=GITHUB_OAUTH_CLIENT_SECRET=... \
     --from-literal=GITHUB_TOKEN=... \
     --from-literal=TWITCH_CLIENT_ID=... \
     --from-literal=TWITCH_CLIENT_SECRET=... \
     --from-literal=YOUTUBE_API_KEY=... \
     --from-literal=ADMIN_GITHUB_LOGINS=cefege
   ```

   The namespace is created by Flux, so run `kubectl create namespace recomphub` first if you create the secret before the first sync.
5. On `ovh1`, apply the Flux bootstrap: `kubectl apply -f deploy/flux/bootstrap.yaml`.
6. Run each worker once, in this order, with `kubectl -n recomphub create job --from=cronjob/<name> <name>-init`: `catalog-sync`, `import-recompendium`, `import-portsdr`, `github-sync`, `media-sync`.
7. Check `curl -s https://recomphub.com/healthz`; it should return `ok`.

## Attribution

Game data and images: IGDB.com. Seed data: Recompendium (MIT).
