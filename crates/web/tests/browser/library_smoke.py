#!/usr/bin/env python3
"""CI-only browser proof for the private saved-games library.

It refuses arbitrary databases and is deliberately run only by the isolated GitHub Actions job.
"""

import hashlib
import os
import time
import urllib.error
import urllib.request

import psycopg
from playwright.sync_api import sync_playwright

BASE = os.environ.get("RECOMPHUB_BROWSER_BASE", "http://127.0.0.1:8080")
DATABASE_URL = os.environ["DATABASE_URL"]
COOKIE = "rh_session"


def refuse_unsafe_environment() -> None:
    if os.environ.get("CI") != "true":
        raise RuntimeError("browser fixture setup is CI-only")
    if not DATABASE_URL.endswith("/recomphub_browser_test"):
        raise RuntimeError("refusing a database other than recomphub_browser_test")


def seed() -> tuple[str, str]:
    refuse_unsafe_environment()
    with psycopg.connect(DATABASE_URL) as conn, conn.cursor() as cur:
        cur.execute("TRUNCATE game_favorites, sessions, users, games CASCADE")
        cur.execute(
            "INSERT INTO games (igdb_id, slug, name) VALUES (90001, 'fixture-a', 'Fixture Game A'), (90002, 'fixture-b', 'Fixture Game B')"
        )
        tokens = []
        for number, name in ((1, "fixture-alice"), (2, "fixture-bob")):
            cur.execute(
                "INSERT INTO users (display_name) VALUES (%s) RETURNING id", (name,)
            )
            user_id = cur.fetchone()[0]
            token = f"ci-browser-token-{number}"
            cur.execute(
                "INSERT INTO sessions (id_hash, user_id, expires_at) VALUES (%s, %s, now() + interval '1 hour')",
                (hashlib.sha256(token.encode()).digest(), user_id),
            )
            tokens.append(token)
        conn.commit()
    return tuple(tokens)


def wait_for_server() -> None:
    for _ in range(60):
        try:
            if urllib.request.urlopen(f"{BASE}/healthz", timeout=1).read() == b"ok":
                return
        except OSError:
            pass
        time.sleep(1)
    raise RuntimeError("container did not become healthy")


def assert_private_cache(token: str) -> None:
    request = urllib.request.Request(
        f"{BASE}/api/v1/library/favorites", headers={"Cookie": f"{COOKIE}={token}"}
    )
    with urllib.request.urlopen(request) as response:
        assert response.headers["Cache-Control"] == "private, no-store"
    missing = urllib.request.Request(
        f"{BASE}/api/v1/library/favorites", headers={"Cookie": f"{COOKIE}=missing"}
    )
    try:
        urllib.request.urlopen(missing)
    except urllib.error.HTTPError as error:
        assert error.code == 401
        assert error.headers["Cache-Control"] == "private, no-store"
    else:
        raise AssertionError("missing session unexpectedly succeeded")


def main() -> None:
    alice, bob = seed()
    wait_for_server()
    assert_private_cache(alice)
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch()
        guest = browser.new_context(viewport={"width": 390, "height": 844})
        # Allow cold debug WebAssembly startup on slow CI workers.
        guest.set_default_timeout(60000)
        guest.route("**/*", lambda route: route.abort() if not route.request.url.startswith(BASE) else route.continue_())
        page = guest.new_page()
        page.goto(f"{BASE}/games/fixture-a")
        button = page.get_by_role("button", name="Save Fixture Game A to your library")
        button.click()
        page.goto(f"{BASE}/library")
        page.get_by_text("Fixture Game A").wait_for()
        page.reload()
        page.get_by_text("Fixture Game A").wait_for()
        assert page.evaluate("document.documentElement.scrollWidth <= window.innerWidth"), "mobile library overflows horizontally"
        page.screenshot(path="artifacts/library-390.png", full_page=True)

        # Installing an existing session cookie makes the device selection merge additively.
        guest.add_cookies([{"name": COOKIE, "value": alice, "url": BASE}])
        page.reload()
        page.get_by_text("Account-synced saved games").wait_for()
        page.get_by_text("Fixture Game A").wait_for()
        page.set_viewport_size({"width": 768, "height": 1024})
        page.screenshot(path="artifacts/library-768.png", full_page=True)
        page.set_viewport_size({"width": 1440, "height": 1000})
        page.screenshot(path="artifacts/library-1440.png", full_page=True)

        # A card may unmount while its account write is pending. The shared
        # provider must finish it and release the pending state on the next route.
        delayed_writes = []
        def delay_favorite_write(route):
            if not delayed_writes:
                delayed_writes.append(route)
            else:
                route.continue_()
        page.route("**/api/v1/library/favorites/set", delay_favorite_write)
        page.get_by_role("button", name="Remove Fixture Game A from your library", exact=True).click()
        page.locator('a[href="/games/fixture-a"]').first.click()
        page.wait_for_url(f"{BASE}/games/fixture-a")
        assert len(delayed_writes) == 1
        delayed_writes[0].continue_()
        page.unroute("**/api/v1/library/favorites/set", delay_favorite_write)
        save = page.get_by_role("button", name="Save Fixture Game A to your library", exact=True)
        save.wait_for()
        assert save.is_enabled()
        save.click()
        page.get_by_role("button", name="Remove Fixture Game A from your library", exact=True).wait_for()

        # The account survives a clean browser context and does not leak to another account.
        clean = browser.new_context()
        clean.set_default_timeout(60000)
        clean.add_cookies([{"name": COOKIE, "value": alice, "url": BASE}])
        clean_page = clean.new_page()
        clean_page.goto(f"{BASE}/library")
        clean_page.get_by_text("Fixture Game A").wait_for()
        other = browser.new_context()
        other.set_default_timeout(60000)
        other.add_cookies([{"name": COOKIE, "value": bob, "url": BASE}])
        other_page = other.new_page()
        other_page.goto(f"{BASE}/library")
        other_page.get_by_text("No saved games yet").wait_for()
        other_page.evaluate("""(url) => {
            history.pushState(history.state, "", url);
            dispatchEvent(new PopStateEvent("popstate", {state: history.state}));
        }""", f"{BASE}/library?page=999")
        other_page.wait_for_url(f"{BASE}/library?page=1")

        # Raw SSR never contains the authenticated selection, and logout returns to device state.
        html = urllib.request.urlopen(f"{BASE}/library").read().decode()
        assert "Fixture Game A" not in html
        clean_page.request.post(f"{BASE}/auth/logout")
        clean_page.reload()
        clean_page.get_by_text("Device-only saved games").wait_for()
        assert clean_page.get_by_text("Fixture Game A").count() == 0

        # History changes reuse SearchPage; the input must follow its route, not its first query.
        history_page = other.new_page()
        history_page.goto(f"{BASE}/search?q=Fixture")
        query = history_page.locator("main input[name='q']")
        assert query.input_value() == "Fixture"
        history_page.get_by_role(
            "button", name="Save Fixture Game A to your library", exact=True
        ).click(trial=True)
        history_page.evaluate("""(url) => {
            history.pushState(history.state, "", url);
            dispatchEvent(new PopStateEvent("popstate", {state: history.state}));
        }""", f"{BASE}/search?q=MissingHistoryProbe")
        history_page.wait_for_function(
            "document.querySelector('main input[name=q]')?.value === 'MissingHistoryProbe'"
        )
        history_page.go_back()
        history_page.wait_for_function(
            "document.querySelector('main input[name=q]')?.value === 'Fixture'"
        )
        browser.close()


if __name__ == "__main__":
    main()
