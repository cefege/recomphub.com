//! Wikipedia best-selling game lists: which lists feed the sales signal, how they are fetched, and
//! how their tables are parsed into [`ListEntry`] rows.
//!
//! Lists come from the MediaWiki parse API, so the rendered tables are the source of truth. Parsing
//! is header-driven: a table qualifies when a header row names a title column and a sales column.
//! Per-year lists additionally need a `Year` column inside a "top-selling" table.

use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use recomphub_db::sales::SalesScope;
use regex::Regex;
use scraper::node::Node;
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;

const API_URL: &str = "https://en.wikipedia.org/w/api.php";
const USER_AGENT: &str = "recomphub/1.0 (+https://recomphub.com)";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Pause between list fetches, so the crawl stays polite to the Wikipedia API.
pub const FETCH_INTERVAL: Duration = Duration::from_secs(1);
/// Upper bound on `rowspan`/`colspan`, so a malformed table cannot blow up the grid.
const MAX_SPAN: usize = 50;

/// A source list and what its rows mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceList {
    /// Canonical English Wikipedia page title.
    pub title: &'static str,
    pub scope: SalesScope,
    /// `platforms.slug` for platform lists; `None` for overall and per-year lists.
    pub platform_slug: Option<&'static str>,
}

const fn on_platform(title: &'static str, slug: &'static str) -> SourceList {
    SourceList {
        title,
        scope: SalesScope::Platform,
        platform_slug: Some(slug),
    }
}

/// Every list that feeds `games.sales_score`.
pub const LISTS: &[SourceList] = &[
    on_platform(
        "List of best-selling Nintendo Entertainment System video games",
        "nes",
    ),
    on_platform(
        "List of best-selling Super Nintendo Entertainment System video games",
        "snes",
    ),
    on_platform("List of best-selling Game Boy video games", "gb"),
    on_platform("List of best-selling Game Boy Advance video games", "gba"),
    on_platform("List of best-selling Nintendo DS video games", "nds"),
    on_platform("List of best-selling Nintendo 3DS video games", "3ds"),
    on_platform("List of best-selling Nintendo 64 video games", "n64"),
    on_platform("List of best-selling GameCube video games", "ngc"),
    on_platform("List of best-selling Wii video games", "wii"),
    on_platform("List of best-selling Wii U video games", "wiiu"),
    on_platform("List of best-selling Nintendo Switch video games", "switch"),
    on_platform(
        "List of best-selling Sega Genesis games",
        "genesis-slash-megadrive",
    ),
    on_platform("List of best-selling Sega Saturn games", "saturn"),
    on_platform("List of best-selling PlayStation video games", "ps"),
    on_platform("List of best-selling PlayStation 2 video games", "ps2"),
    on_platform("List of best-selling PlayStation 3 video games", "ps3"),
    on_platform("List of best-selling PlayStation 4 video games", "ps4--1"),
    on_platform("List of best-selling PlayStation 5 video games", "ps5"),
    on_platform(
        "List of best-selling PlayStation Portable video games",
        "psp",
    ),
    on_platform("List of best-selling Xbox games", "xbox"),
    on_platform("List of best-selling Xbox 360 video games", "xbox360"),
    on_platform("List of best-selling Xbox One video games", "xboxone"),
    on_platform("List of best-selling PC games", "win"),
    SourceList {
        title: "List of best-selling video games",
        scope: SalesScope::Overall,
        platform_slug: None,
    },
    SourceList {
        title: "List of best-selling Nintendo video games",
        scope: SalesScope::Overall,
        platform_slug: None,
    },
    SourceList {
        title: "List of best-selling video games in the United States by year",
        scope: SalesScope::Year,
        platform_slug: None,
    },
];

/// One row of a best-selling list.
#[derive(Debug, Clone, PartialEq)]
pub struct ListEntry {
    /// Title as printed in the list, with footnote and dagger marks removed.
    pub title: String,
    pub rank: Option<i32>,
    /// Sales year, only for per-year lists.
    pub sales_year: Option<i32>,
    pub units_sold_millions: f64,
}

/// A list together with the outcome of fetching and parsing it.
pub struct FetchedList {
    pub list: SourceList,
    pub entries: Result<Vec<ListEntry>>,
}

/// The HTTP client used for Wikipedia requests.
pub fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .context("building the Wikipedia HTTP client")
}

/// Public article URL for a list title.
pub fn page_url(title: &str) -> String {
    format!("https://en.wikipedia.org/wiki/{}", title.replace(' ', "_"))
}

/// Fetches and parses every list in [`LISTS`], one at a time with [`FETCH_INTERVAL`] between them.
/// A failing list is reported in its own result and does not stop the others.
pub async fn fetch_all(http: &reqwest::Client) -> Vec<FetchedList> {
    let mut out = Vec::with_capacity(LISTS.len());
    for (index, list) in LISTS.iter().copied().enumerate() {
        if index > 0 {
            tokio::time::sleep(FETCH_INTERVAL).await;
        }
        let entries = match fetch_html(http, list.title).await {
            Ok(html) => parse_entries(&html, list.scope),
            Err(err) => Err(err),
        };
        out.push(FetchedList { list, entries });
    }
    out
}

/// Returns the rendered HTML of a page, following redirects.
pub async fn fetch_html(http: &reqwest::Client, title: &str) -> Result<String> {
    let body: Value = http
        .get(API_URL)
        .query(&[
            ("action", "parse"),
            ("page", title),
            ("prop", "text"),
            ("format", "json"),
            ("formatversion", "2"),
            ("redirects", "1"),
            ("disabletoc", "1"),
        ])
        .send()
        .await
        .with_context(|| format!("requesting {title}"))?
        .error_for_status()
        .with_context(|| format!("fetching {title}"))?
        .json()
        .await
        .with_context(|| format!("decoding {title}"))?;
    if let Some(err) = body.get("error") {
        bail!("Wikipedia returned an error for {title}: {err}");
    }
    body.pointer("/parse/text")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("no parsed text for {title}"))
}

/// Parses every qualifying table in a rendered page into rows.
///
/// Fails when no table qualifies, which usually means the page layout changed.
pub fn parse_entries(html: &str, scope: SalesScope) -> Result<Vec<ListEntry>> {
    static TABLE: LazyLock<Selector> =
        LazyLock::new(|| Selector::parse("table.wikitable").expect("valid selector"));

    let doc = Html::parse_fragment(html);
    let mut entries = Vec::new();
    let mut qualifying = 0usize;
    for table in doc.select(&TABLE) {
        let grid = build_grid(table);
        let Some(header) = find_header(&grid) else {
            continue;
        };
        let Some(columns) = columns(&grid[header], scope) else {
            continue;
        };
        qualifying += 1;
        entries.extend(rows(&grid[header + 1..], &columns, scope));
    }
    if qualifying == 0 {
        bail!("no {scope:?} sales table found");
    }
    Ok(entries)
}

/// Column positions resolved from a header row.
struct Columns {
    title: usize,
    sales: usize,
    rank: Option<usize>,
    year: Option<usize>,
    /// The sales header names millions, so bare numbers are millions.
    sales_in_millions: bool,
}

fn is_title_header(cell: &str) -> bool {
    let cell = cell.trim().to_lowercase();
    cell.contains("title") || cell.starts_with("game")
}

fn is_sales_header(cell: &str) -> bool {
    let cell = cell.trim().to_lowercase();
    cell.contains("sales") || cell.contains("copies")
}

fn is_rank_header(cell: &str) -> bool {
    let cell = cell.trim().to_lowercase();
    cell == "rank" || cell == "no." || cell == "#"
}

/// Index of the first row that names both a title and a sales column.
fn find_header(grid: &[Vec<String>]) -> Option<usize> {
    grid.iter().position(|row| {
        row.iter().any(|c| is_title_header(c)) && row.iter().any(|c| is_sales_header(c))
    })
}

fn columns(header: &[String], scope: SalesScope) -> Option<Columns> {
    let title = header.iter().position(|c| is_title_header(c))?;
    let sales = header.iter().position(|c| is_sales_header(c))?;
    let year = header
        .iter()
        .position(|c| c.trim().eq_ignore_ascii_case("year"));
    if scope == SalesScope::Year
        && (year.is_none() || !header[title].to_lowercase().contains("top-selling"))
    {
        return None;
    }
    let sales_header = header[sales].to_lowercase();
    Some(Columns {
        title,
        sales,
        rank: header.iter().position(|c| is_rank_header(c)),
        year,
        sales_in_millions: sales_header.contains("million") || sales_header.contains("mln"),
    })
}

fn rows(body: &[Vec<String>], columns: &Columns, scope: SalesScope) -> Vec<ListEntry> {
    let mut out = Vec::new();
    for row in body {
        let (Some(title_cell), Some(sales_cell)) = (row.get(columns.title), row.get(columns.sales))
        else {
            continue;
        };
        let Some(units_sold_millions) = parse_units(sales_cell, columns.sales_in_millions) else {
            continue;
        };
        let sales_year = match scope {
            SalesScope::Year => {
                let year = columns
                    .year
                    .and_then(|i| row.get(i))
                    .and_then(|c| parse_year(c));
                if year.is_none() {
                    continue;
                }
                year
            }
            SalesScope::Platform | SalesScope::Overall => None,
        };
        let rank = columns
            .rank
            .and_then(|i| row.get(i))
            .and_then(|c| parse_rank(c));
        for line in title_cell.lines() {
            if let Some(title) = clean_title(line) {
                out.push(ListEntry {
                    title,
                    rank,
                    sales_year,
                    units_sold_millions,
                });
            }
        }
    }
    out
}

/// Expands a table into a dense grid of cell texts, repeating spanned cells across their span.
fn build_grid(table: ElementRef) -> Vec<Vec<String>> {
    static TR: LazyLock<Selector> =
        LazyLock::new(|| Selector::parse("tr").expect("valid selector"));

    let mut rows: Vec<Vec<String>> = Vec::new();
    // Per column: text of a cell still spanning down, and how many more rows it covers.
    let mut carry: Vec<Option<(String, usize)>> = Vec::new();
    for tr in table.select(&TR) {
        if !belongs_to(tr, table) {
            continue;
        }
        let mut row: Vec<String> = Vec::new();
        let mut col = 0usize;
        for cell in tr
            .children()
            .filter_map(ElementRef::wrap)
            .filter(|c| matches!(c.value().name(), "td" | "th"))
        {
            col = fill_carried(&mut carry, &mut row, col);
            let text = cell_text(cell);
            let rowspan = span_attr(cell, "rowspan");
            let colspan = span_attr(cell, "colspan");
            for _ in 0..colspan {
                row.push(text.clone());
                if rowspan > 1 {
                    set_carry(&mut carry, col, Some((text.clone(), rowspan - 1)));
                }
                col += 1;
            }
        }
        fill_carried(&mut carry, &mut row, col);
        rows.push(row);
    }
    rows
}

/// Pushes carried-over cells into `row` starting at `col`. Returns the column after the last one.
fn fill_carried(
    carry: &mut [Option<(String, usize)>],
    row: &mut Vec<String>,
    mut col: usize,
) -> usize {
    while let Some(Some((text, left))) = carry.get(col).cloned() {
        row.push(text.clone());
        carry[col] = (left > 1).then(|| (text, left - 1));
        col += 1;
    }
    col
}

fn set_carry(carry: &mut Vec<Option<(String, usize)>>, col: usize, value: Option<(String, usize)>) {
    if carry.len() <= col {
        carry.resize(col + 1, None);
    }
    carry[col] = value;
}

/// True when the nearest enclosing table of `tr` is `table` (skips rows of nested tables).
fn belongs_to(tr: ElementRef, table: ElementRef) -> bool {
    tr.ancestors()
        .filter_map(ElementRef::wrap)
        .find(|e| e.value().name() == "table")
        .is_some_and(|e| e.id() == table.id())
}

fn span_attr(cell: ElementRef, name: &str) -> usize {
    cell.value()
        .attr(name)
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, MAX_SPAN)
}

/// Visible text of a cell. Footnotes, edit links and styles are dropped; `<br>` and block
/// elements become line breaks so that multi-title cells can be split.
fn cell_text(cell: ElementRef) -> String {
    let mut out = String::new();
    push_text(cell, &mut out);
    out.replace('\u{a0}', " ")
}

fn push_text(element: ElementRef, out: &mut String) {
    for child in element.children() {
        match child.value() {
            Node::Text(text) => out.push_str(text),
            Node::Element(el) => {
                let name = el.name();
                if name == "br" {
                    out.push('\n');
                    continue;
                }
                let skipped = matches!(name, "sup" | "style" | "script")
                    || el
                        .classes()
                        .any(|c| matches!(c, "reference" | "mw-editsection" | "noprint"));
                if skipped {
                    continue;
                }
                let block = matches!(name, "li" | "p" | "div");
                if block {
                    out.push('\n');
                }
                if let Some(child) = ElementRef::wrap(child) {
                    push_text(child, out);
                }
                if block {
                    out.push('\n');
                }
            }
            _ => {}
        }
    }
}

static FOOTNOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[[^\]]*\]").expect("valid regex"));
static UNITS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(\d{1,3}(?:,\d{3})+|\d+)(\.\d+)?\s*(million|mln|billion|bn)?\+?$")
        .expect("valid regex")
});
static YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:19|20)\d{2}\b").expect("valid regex"));
static DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").expect("valid regex"));

/// Cleans a title line: drops footnotes, daggers and extra whitespace. `None` if nothing is left.
pub fn clean_title(line: &str) -> Option<String> {
    let without_notes = FOOTNOTE.replace_all(line, "");
    let collapsed = without_notes
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let title = collapsed.trim_end_matches(['†', '‡', '*']).trim();
    (!title.is_empty()).then(|| title.to_owned())
}

/// Parses a sales cell into millions of units.
///
/// The first non-empty line must be a bare number with an optional `million`, `mln`, `billion` or
/// `bn` unit, optionally followed by `+`. Anything else (`—`, `2010–2019`, prose) is `None`, which
/// is how section-header rows are skipped. A bare number with no unit is millions when the header
/// says so; otherwise a value of at least 1000 is taken as raw copies.
pub fn parse_units(cell: &str, header_in_millions: bool) -> Option<f64> {
    let line = cell.lines().map(str::trim).find(|l| !l.is_empty())?;
    let cleaned = FOOTNOTE.replace_all(line, "").replace('\u{a0}', " ");
    let text = cleaned.trim().trim_end_matches(['†', '‡', '*']).trim();
    let caps = UNITS.captures(text)?;
    let integer = caps.get(1)?.as_str().replace(',', "");
    let fraction = caps.get(2).map_or("", |m| m.as_str());
    let value: f64 = format!("{integer}{fraction}").parse().ok()?;
    let value = match caps.get(3).map(|m| m.as_str().to_ascii_lowercase()) {
        Some(unit) if unit == "billion" || unit == "bn" => value * 1000.0,
        Some(_) => value,
        None if header_in_millions => value,
        None if value >= 1000.0 => value / 1_000_000.0,
        None => value,
    };
    Some(value)
}

fn parse_year(cell: &str) -> Option<i32> {
    YEAR.find(cell)?.as_str().parse().ok()
}

fn parse_rank(cell: &str) -> Option<i32> {
    DIGITS.find(cell)?.as_str().parse().ok()
}
