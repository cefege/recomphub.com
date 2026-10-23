# RecompHub

RecompHub (recomphub.com) is a searchable database of console and PC game recompilations and native ports. Each game has a page listing its known ports, where they run, their authors, AI usage, GitHub stars, release status and gameplay videos.

## Saved games library

`/library` is a private game-bookmark library, not an installed-games list. Guests can save existing catalog games in browser local storage. On sign-in, valid device favorites are additively imported into the account; existing account favorites are never replaced. Signing out does not copy account favorites back to the browser. Account favorites are private, have no public count or activity entry, and are removed automatically if the account or catalog game is deleted.

The website remains a catalog and account service. It does not install or launch games, verify downloaded binaries, or supply original-game data. The native desktop client shares its catalog, account APIs, components and design tokens while keeping installations and original data on the user's device. Desktop CI packages are development artifacts, not public releases.

## Native desktop builds — Mac ARM and Windows x64

The supported desktop targets are macOS Apple Silicon (`aarch64-apple-darwin`) and Windows x64 (`x86_64-pc-windows-msvc`). Intel Mac is not a delivery target.

Customers install the desktop client only. Postgres, OAuth credentials and ingestion workers stay on the website server. **Sign-in is optional:** the library, public catalog, managed installs, game-data setup, launch, repair, updates and uninstall do not require an account. Catalog reads and new downloads require a network connection, not an authenticated session.

The native workspace has a persistent installed-games sidebar, local filtering and selection, and a selected-game action panel. Saved games, catalog/search/detail pages, media and action controls reuse the website's components and data rather than a separate catalog or copied implementation.

Signed-out and offline saved-game selections persist in native SQLite, independently of WebView storage. Optional **Continue with Google** connects the existing RecompHub account and additively syncs device selections; only server-acknowledged IDs are removed from the pending device selection. Account credentials stay in macOS Keychain or Windows Credential Manager. Account errors, sign-out and expiry never lock local games or cancel their operations.

Installations use device-local SQLite and managed package directories. Game updates require an explicit click; rollback restores the previous package and its repair metadata. Original game files, saves, configuration and backups stay outside package removal. RecompHub does not supply ROMs or bypass Gatekeeper or SmartScreen. A normal macOS assessment refusal is shown as a launch error, not a false Running state.

The library is per OS user: `~/Library/Application Support/RecompHub` on macOS and `%LOCALAPPDATA%\RecompHub` on Windows. The current-user Windows installer puts the client under `%LOCALAPPDATA%\Programs`, separate from managed games.

Reviewed managed packages currently cover Banjo: Recompiled on macOS Intel/Apple Silicon and Windows x64. Other catalog entries keep their upstream links without a guessed installer. The flat macOS app layout is present in v1.0.2; the published v1.0.0 and v1.0.1 archives wrap another ZIP and do not match that reviewed executable layout.

Install the pinned Dioxus CLI version before bundling:

```sh
rustup run stable cargo install dioxus-cli --version 0.7.10 --locked
```

On an Apple Silicon Mac, build the app bundle and DMG into fresh output directories. Replacing a previously launched executable in place can leave stale macOS launch/signature state. The configured `-` identity seals local development bundles and their resources without selecting a developer certificate; it is not Developer ID signing or notarization.
The DMG helper selects non-indexed APFS explicitly, then mounts each image read-only and checks the enclosed app signature and arm64-only executable. It does not retry image-tool failures or alter app signatures.

```sh
mkdir -p target/desktop-artifacts
macos_debug="$(mktemp -d target/desktop-artifacts/macos-debug.XXXXXX)"
macos_release="$(mktemp -d target/desktop-artifacts/macos.XXXXXX)"
env -u APPLE_CERTIFICATE -u APPLE_ID -u APPLE_API_KEY rustup run stable "$HOME/.cargo/bin/dx" bundle --desktop --target aarch64-apple-darwin --no-default-features --features desktop --package recomphub --fullstack false --codesign false --package-types macos --out-dir "$macos_debug"
env -u APPLE_CERTIFICATE -u APPLE_ID -u APPLE_API_KEY rustup run stable "$HOME/.cargo/bin/dx" bundle --desktop --target aarch64-apple-darwin --no-default-features --features desktop --release --package recomphub --fullstack false --codesign false --package-types macos --out-dir "$macos_release"
codesign --verify --deep --strict "$macos_debug/RecompHub.app"
codesign --verify --deep --strict "$macos_release/RecompHub.app"
python3 .github/package_macos_dmg.py "$macos_debug/RecompHub.app" "$macos_debug"
python3 .github/package_macos_dmg.py "$macos_release/RecompHub.app" "$macos_release"
```

On Windows x64, build the current-user NSIS installer:

```powershell
rustup run stable "$HOME/.cargo/bin/dx.exe" bundle --desktop --no-default-features --features desktop --package recomphub --fullstack false --codesign false --package-types nsis --out-dir target/desktop-artifacts/windows-debug
rustup run stable "$HOME/.cargo/bin/dx.exe" bundle --desktop --no-default-features --features desktop --release --package recomphub --fullstack false --codesign false --package-types nsis --out-dir target/desktop-artifacts/windows
```

Windows x64 builds statically link the C runtime through `.cargo/config.toml`; no separate Visual C++ redistributable is required. Preserve `-C target-feature=+crt-static` when overriding target Rust flags. The NSIS installer includes the WebView2 bootstrapper.

Native CI targets Mac ARM and Windows x64 only. It runs engine lifecycle and account-state checks, excludes database/ingestion/SQLx/server-renderer dependencies from the client, verifies Mac bundle signatures, and uploads local development debug/release artifacts. Mac artifacts are ad-hoc signed, not Developer ID signed or notarized; Windows artifacts are unsigned. Intel Mac is not built.

Treat implementation, runtime verification and distribution readiness as separate gates. Implementation requires the behavioral suites and native acceptance scenarios to pass. Runtime verification is recorded separately for each operating system and scenario. Distribution readiness also requires the authorized production backend, signing and notarization where applicable. CI artifacts and successful builds alone do not establish runtime verification or distribution readiness.

Debug builds accept `RECOMPHUB_API_URL=http://localhost:8080` and `RECOMPHUB_LIBRARY_ROOT` for an isolated local backend and library. Release builds ignore both overrides and use `https://recomphub.com` with the normal device library directory. These variables belong to the app process, not its build command alone.

The site is a Rust workspace:

- `crates/shared`: serde DTOs, enums, slug helpers and SEO text builders (compiles to wasm).
- `crates/db`: sqlx Postgres pool, migrations, queries and submission logic.
- `crates/desktop`: native package lifecycle, device SQLite, OS credentials and process tracking.
- `crates/ingest`: IGDB, GitHub, Reddit and seed-import clients, AI and platform detection, media pipeline.
- `crates/web`: the shared Dioxus website and native desktop UI (web server binary `server` when built with `dx`).
- `crates/worker`: the `worker` CLI that runs sync and import jobs.

## Local development

```sh
brew services start postgresql@18
createdb recomphub
createdb recomphub_test
cp .env.example .env        # then fill in the keys listed below
rustup run stable "$HOME/.cargo/bin/dx" serve --web --no-default-features --package recomphub
```

Let `dx` select each renderer's feature. Do not pass `--features web` as shared fullstack arguments: Dioxus 0.7.10 also enables it on the server, silently dropping SSR hydration data ([upstream issue #5427](https://github.com/dioxuslabs/dioxus/issues/5427)). Explicit overrides must use separate `@client` and `@server` arguments. The crate rejects mixed renderer features.

Run the worker commands from the repository root:

```sh
cargo run -p recomphub-worker -- catalog-sync
cargo run -p recomphub-worker -- import-recompendium
cargo run -p recomphub-worker -- import-portsdr
cargo run -p recomphub-worker -- github-sync
cargo run -p recomphub-worker -- media-sync
cargo run -p recomphub-worker -- media-discovery
cargo run -p recomphub-worker -- discover
cargo run -p recomphub-worker -- triage-candidates --dry-run --limit 100
cargo run -p recomphub-worker -- triage-candidates --offline crates/ingest/tests/fixtures/candidate-triage.json
cargo run -p recomphub-worker -- describe-ports --dry-run
cargo run -p recomphub-worker -- describe-games --dry-run --game need-for-speed-most-wanted
cargo run -p recomphub-worker -- stats-snapshot
cargo run -p recomphub-worker -- sales-sync
cargo run -p recomphub-worker --bin discord-setup -- --dry-run --prune --server-icon
cargo run -p recomphub-worker --bin discord-setup -- --prune --server-icon
```
`discord-setup` maintains five community channels: `#welcome`, `#announcements`, `#general`, `#showcase`, and `#website-bugs`. Their channel topics supply onboarding and posting guidance. `--server-icon` applies the bundled RecompHub logo; `--prune` deletes every other channel and category, so use it only for an intentional clean cutover.

`triage-candidates` prepares evidence-backed moderation context; it does not publish or reject submissions. Normal mode requires `DATABASE_URL`, `GITHUB_TOKEN`, `SECRETS_KEY`, and configured enabled AI providers. `--limit` accepts 1–100 and defaults to 100; it caps both preparations and model completion attempts, including provider fallbacks. Repeat `--id <submission-id>` to deliberately select a calibration/retry set. `--dry-run` requires only `DATABASE_URL`: it lists due rows and cached prompts without fetching GitHub evidence, calling a model, or writing data. `--offline <path>` reads a fixture only, requires no database, credentials, or HTTP client, and validates supplied untrusted responses; it does not exercise AI. The exact offline fixture command is shown above. Live commands access external services or write records and require separate explicit operational authorization.


Tests:

```sh
DATABASE_URL=postgres://localhost/recomphub_test cargo test --workspace --features recomphub/server
```

## Advertising

The shared document head in `crates/web/src/app.rs` loads Google AdSense asynchronously for publisher `ca-pub-6004542868270159`, with `crossorigin="anonymous"`.
The root `/ads.txt` endpoint authorizes Google as a direct seller with `google.com, pub-6004542868270159, DIRECT, f08c47fec0942fa0`.

Consent defaults deny analytics storage, ad storage, ad user data and ad personalization before the Google tags load. Google Analytics remains disabled until Zaraz purpose `pUSs` is accepted and is disabled again on withdrawal. Zaraz analytics permission never grants advertising permission. The privacy page and footer expose separate analytics and advertising settings.

Google's certified consent message must be configured in the **AdSense account**, not replaced with the Zaraz analytics banner:

1. Deploy the updated privacy page, then open **Privacy & messaging → European regulations** in AdSense.
2. Create a message for `recomphub.com`, set `https://recomphub.com/privacy` as the privacy URL, enable **Do not consent**, and publish it.
3. Enable **Consent mode for advertising purposes** in the message settings. Leave its separate analytics consent mode off: Zaraz remains the analytics consent source.
4. Verify the published message, decline and withdrawal on the deployed site. Google's message preview query is `?fc=alwaysshow&fctype=gdpr`; do not mistake a preview for production-region verification. The existing AdSense script loads the published message; no second CMP script is required.

Publishing the message requires authenticated account access. Until it is published and loads, the advertising settings control reports that it is unavailable; the repository does not contain an alternative certified CMP.

### Catalog review and copyright

Imported game descriptions are attributed to IGDB. FAQ comparisons describe recorded targets and community playability labels, not independent testing or editorial recommendations. Generated FAQs use the `facts-v2:` source-hash prefix; older cached answers are excluded until the existing ingest job regenerates them. This cutover requires no migration or manual database write.

About, report dialogs and port download panels explain external releases, original game assets and copyright reporting. Reports can be emailed to `mateiasmihaiandrei@gmail.com` without signing in. These disclosures do not verify linked files or replace editorial review. Every member-submitted new port stays pending for evidence triage and moderator approval before publication, regardless of contributor history.

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
| `DATAFORSEO_LOGIN` | DataForSEO account login; required by the paused `media-discovery` worker |
| `DATAFORSEO_PASSWORD` | DataForSEO API password; required by the paused `media-discovery` worker |
| `SECRETS_KEY` | 32 bytes, base64 (`openssl rand -base64 32`); encrypts AI provider credentials at rest. Web and workers need the same value |
| `GOOGLE_OAUTH_CLIENT_ID` | Google OAuth client ID (Web application); the only sign-in method. `/auth/google` returns 503 without it. Redirect URI: `${SITE_URL}/auth/google/callback` |
| `GOOGLE_OAUTH_CLIENT_SECRET` | Google OAuth client secret |
| `ADMIN_EMAILS` | comma-separated; a Google sign-in whose verified email is listed gets the admin role |
| `RUST_LOG` | |
| `DISCORD_BOT_TOKEN` | Discord bot token for the one-shot `discord-setup` command; grants the bot API access to configure the RecompHub server |
| `IP` and `PORT` | set by the Dockerfile; dx reads them |

## Container and deployment

- `Dockerfile` builds the site with `dx bundle --web --release --package recomphub` and the worker with `cargo build --release -p recomphub-worker`. The runtime image puts the bundle in `/usr/local/app/` (entrypoint `server`) and the worker at `/usr/local/app/worker`. If the bundle path printed by `dx bundle` differs from `target/dx/recomphub/release/web/`, update the `COPY` line in the Dockerfile.
- Desktop production activation is separate from local builds. After approval, deploy the tested server/worker image through the existing image/Flux workflow; server bootstrap applies SQLx migrations. Then run `/usr/local/app/worker launcher-sync` and verify desktop authorization headers and host-filtered offers before distributing a production-origin app. A main push can trigger deployment; do not use it as a smoke test.
- CI (`.github/workflows/ci.yml`) runs fmt, clippy and tests against a `postgres:17-alpine` service.
- `discover` is intake: it searches GitHub and scans the newest 100 posts from each of r/recomps, r/decomps and r/macgaming for linked GitHub repositories, placing distinct new repositories in the pending candidate queue. `triage-candidates` is the separate evidence-preparation stage: it fetches bounded repository evidence, proposes a category and review hints, and records citations for moderators. Popularity, releases, recent activity, archive status, or unfinished work are not admission requirements; substantive game-specific implementation or documented progress can qualify even when source-only or archived. Missing or insufficient evidence remains uncertain rather than being automatically rejected.
- The `candidate-triage` CronJob is intentionally paused (`suspend: true`), with schedule `15 */6 * * *` (four times daily), `Forbid` concurrency, at most 100 preparations and 100 completion attempts per run including provider fallbacks. Keep it paused through the reviewed calibration process below. Candidates remain pending until human moderators publish or reject them using the server-filtered, 50-row review pages; triage never decides lifecycle outcomes.
- `media-discovery` (CronJob `media-discovery`, daily 03:00 UTC) is intentionally paused. It submits bounded normal-priority Google Organic tasks to DataForSEO and later collects only task IDs recorded by RecompHub. Candidates are limited to ordinary YouTube watch URLs. Rank 1–3 results that name the game and port are published; ambiguous results stay in moderator review. It never opens YouTube pages or uses browser cookies. Enable it only after `DATAFORSEO_LOGIN` and `DATAFORSEO_PASSWORD` are configured.
- `github-sync`'s weekly AI/author scan also reads each port's README for screenshots: markdown and `<img>` images, filtered of badges, shields, stat cards and logos (`recomphub_ingest::readme_images`), resolved to absolute URLs and capped at 6 per port. They enter moderation as pending `port_media` rows after rendering validates their dimensions; only approved images show in the port gallery.
- Public media is immutable named AVIF at `/media/{public-filename}`; `?w=` accepts only a recorded rendition width and the largest rendition is canonical without a query. The `media-worker` Deployment continuously claims and renders pending media, retries failures with durable bounded backoff, and releases its advisory claim automatically on restart. `worker media-migrate` remains the explicit serial v2 repair command; `--id <media-id>` repairs an audited row. Existing hash, versioned, and OG JPEG paths redirect once to the named URL.
- `sales-sync` (CronJob `sales-sync`, Mondays 04:00 UTC) matches Wikipedia's best-selling game lists (per platform, overall, and per year) to catalog games. Each game's `sales_score` is `round(300 * log2(1 + best units in millions))` across its matches, so a million-seller sits near the 90th percentile of IGDB popularity. `games.importance` is the generated column `popularity + sales_score`, and all catalog ordering uses it. A list that fails to fetch or parse keeps its previous matches and fails the run. `cargo run -p recomphub-worker -- sales-sync --dry-run` prints the parsed rows without writing.
- `describe-ports` (CronJob `describe-ports`, daily 06:30 UTC) writes a short description of each port from the repository text that `github-sync` stores: README, topics and latest release notes. It uses the AI providers configured at `/admin` → AI providers (Anthropic or OpenAI API keys, or Claude / ChatGPT subscription logins), tried in the order shown there with fallback to the next on any failure, and accepts only output that matches the schema and names no ROM download site. Each run makes at most 200 calls, least recently attempted first, and regenerates a description only when its stored text or port facts change. With no enabled provider, or without `SECRETS_KEY`, the job logs and exits. `describe-ports --dry-run` prints the next request and calls nothing.
- `describe-games` (CronJob `describe-games`, daily 06:45 UTC) writes each game page's FAQ from the facts the database holds: original platforms, ports, decompilations, release, maintenance level, authors and requirements. It runs only for games with a port or decompilation, uses the same AI providers as `describe-ports`, and stores the first answer that passes the checks (3 to 7 items, no yes/no openers, no names or numbers missing from the facts, no URLs or ROM-source wording). Otherwise the page keeps the built-in FAQ. Each run makes at most 60 calls, and a game whose last attempt failed on the same facts is retried after 3 days. `describe-games --dry-run --game <slug>` prints the prompt, facts and schema and calls nothing.
- `stats-snapshot` (CronJob `stats-snapshot`, daily 03:30 UTC) stores today's platform parity counters in `stats_daily` (source `snapshot`, one row per day, rewritten if the job runs again that day). It then rebuilds every earlier day from each project's GitHub repository creation date, or the day it was added when GitHub has no date (source `backfill`, replaced on each run). The `/stats` page reads these rows.
- `.github/workflows/image.yml` builds and pushes `ghcr.io/cefege/recomphub.com:sha-<12>` on each push to `main`, then pins the tag in `deploy/production/kustomization.yaml`.
- Flux applies `deploy/production/` on the `ovh1` k3s cluster through `deploy/flux/bootstrap.yaml`.

- `bin/kube <kubectl arguments...>` is the repeatable production control path. It runs `sudo -n /usr/local/bin/kubectl` through the existing `ovh1` SSH alias; it neither needs a local Kubernetes installation nor copies a kubeconfig.

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
     --from-literal=DATAFORSEO_LOGIN=... \
     --from-literal=DATAFORSEO_PASSWORD=... \
     --from-literal=SECRETS_KEY="$(openssl rand -base64 32)" \
     --from-literal=ADMIN_EMAILS=mateiasmihaiandrei@gmail.com
   ```

   The namespace is created by Flux, so run `kubectl create namespace recomphub` first if you create the secret before the first sync.
5. On `ovh1`, apply the Flux bootstrap: `kubectl apply -f deploy/flux/bootstrap.yaml`. Then run `kubectl -n recomphub create job --from=cronjob/postgres-backup postgres-backup-init` once: the `local-path` backup volume binds only when a pod first uses it, and Flux reports the Kustomization unhealthy until it is bound.
6. Run each worker once, in this order, with `kubectl -n recomphub create job --from=cronjob/<name> <name>-init`: `catalog-sync`, `import-recompendium`, `import-portsdr`, `github-sync`, `sales-sync`, `media-sync`.
7. Check `curl -s https://recomphub.com/healthz`; it should return `ok`.
8. Candidate preparation requires a separate, explicitly authorized operational run. First deploy the implementation and apply its migration through the normal approved deployment path; the triage worker does not run migrations. Keep `candidate-triage` suspended. Ensure the secret supplies `DATABASE_URL`, `GITHUB_TOKEN`, and `SECRETS_KEY`, and configure enabled AI providers in the application; dry-run needs only `DATABASE_URL`. With separate authorization for production access, run `worker triage-candidates --dry-run --limit 100`; verify due counts and confirm that lifecycle, triage, and provider records are unchanged. Dry-run only inspects database state and cached evidence; it does not scan GitHub.
9. With explicit authorization for GitHub/provider access and triage writes, run a calibration batch limited to 20 deliberately selected rows using `worker triage-candidates --limit 20 --id <ID> ...`. Include member submissions, source-only/no-release and archived work, likely tools/lists, sparse evidence, and conflicting claims. Inspect all results, citations, provider/model provenance, completion-request count and fetched timestamps. The only permitted changes are triage context and provider credential/health records; approvals, rejections, and trust must remain unchanged. If fallbacks exhaust the completion budget, rerun the same explicit IDs: successful cached assessments are reused and unfinished work is processed within the same per-run limit.
10. If any genuine unfinished/archived game project is treated as unrelated due to incompleteness, stars, or age, or a citation is fabricated, keep the schedule suspended, correct the prompt/validator, bump `TRIAGE_VERSION`, and rerun the sample. Once the policy check passes, obtain explicit operator confirmation before changing `suspend` to `false` in the deployment repository. Then process the backlog only through the same bounded command, not an unbounded script. Every pending lead remains reachable as classified, uncertain, untriaged, or failed.
11. Moderators review the member priority lane first, then game projects, and reject selected unrelated/supporting entries only with human-entered reasons. Tools and references remain in pending/rejected records, not deleted or auto-published. Uncertain and failed items stay pending for investigation; reducing queue size is not a reason to reject them.

## Search data access

- Search Console analysis uses separate read-only OAuth credentials, not the site's Google sign-in client. The local authorized-user credential file is `~/.config/recomphub/google-search-console.json` (permissions `0600`), with scope `https://www.googleapis.com/auth/webmasters.readonly` and property `sc-domain:recomphub.com`. It includes a refresh token; keep it outside the repository.
- For tools using Google Application Default Credentials, set `GOOGLE_APPLICATION_CREDENTIALS` to that file's absolute path. The web app and worker do not currently consume these credentials.
- The official [Google Trends API](https://developers.google.com/search/apis/trends) requires alpha approval. A generic Google Cloud API key does not grant access.

## Attribution

Seed data: Recompendium (MIT).
