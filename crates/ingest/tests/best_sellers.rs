use recomphub_db::sales::SalesScope;
use recomphub_ingest::best_sellers::{LISTS, ListEntry, clean_title, parse_entries, parse_units};

const N64: &str = include_str!("fixtures/best_sellers/n64.html");
const WII: &str = include_str!("fixtures/best_sellers/wii.html");
const NINTENDO_MLN: &str = include_str!("fixtures/best_sellers/nintendo_mln.html");
const SWITCH: &str = include_str!("fixtures/best_sellers/switch.html");
const PC: &str = include_str!("fixtures/best_sellers/pc.html");
const OVERALL: &str = include_str!("fixtures/best_sellers/overall.html");
const US_BY_YEAR: &str = include_str!("fixtures/best_sellers/us_by_year.html");
const US_NUMBERED_SKIP: &str = include_str!("fixtures/best_sellers/us_numbered_skip.html");

fn entry<'a>(entries: &'a [ListEntry], title: &str) -> &'a ListEntry {
    entries
        .iter()
        .find(|e| e.title == title)
        .unwrap_or_else(|| panic!("no entry titled {title:?} in {entries:?}"))
}

fn assert_millions(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected} million, got {actual}"
    );
}

#[test]
fn n64_raw_copies_are_converted_to_millions() {
    let entries = parse_entries(N64, SalesScope::Platform).unwrap();
    let mario = entry(&entries, "Super Mario 64");
    assert_millions(mario.units_sold_millions, 11.91);
    assert_eq!(mario.rank, None);
    assert_eq!(mario.sales_year, None);
    assert!(entries.iter().all(|e| !e.title.contains('†')));
}

#[test]
fn copies_sold_header_with_million_units() {
    let entries = parse_entries(WII, SalesScope::Platform).unwrap();
    assert_millions(entry(&entries, "Wii Sports").units_sold_millions, 82.9);
    assert_millions(entry(&entries, "Mario Kart Wii").units_sold_millions, 37.38);
}

#[test]
fn mln_header_is_millions_for_bare_numbers() {
    let entries = parse_entries(NINTENDO_MLN, SalesScope::Overall).unwrap();
    assert_millions(entry(&entries, "Wii Sports").units_sold_millions, 82.9);
    assert_millions(
        entry(&entries, "Mario Kart 8 Deluxe").units_sold_millions,
        71.53,
    );
}

#[test]
fn switch_table_units() {
    let entries = parse_entries(SWITCH, SalesScope::Platform).unwrap();
    assert_millions(
        entry(&entries, "Super Smash Bros. Ultimate").units_sold_millions,
        38.14,
    );
}

#[test]
fn pc_total_copies_sold() {
    let entries = parse_entries(PC, SalesScope::Platform).unwrap();
    assert_millions(
        entry(&entries, "PUBG: Battlegrounds").units_sold_millions,
        41.0,
    );
}

#[test]
fn overall_list_reads_rank_and_nbsp_titles() {
    let entries = parse_entries(OVERALL, SalesScope::Overall).unwrap();
    let tetris = entry(&entries, "Tetris");
    assert_eq!(tetris.rank, Some(1));
    assert_millions(tetris.units_sold_millions, 520.0);
    let rdr2 = entry(&entries, "Red Dead Redemption 2");
    assert_eq!(rdr2.rank, Some(3));
    assert_millions(rdr2.units_sold_millions, 87.0);
}

#[test]
fn year_list_keeps_sales_year_and_rowspan_titles() {
    let entries = parse_entries(US_BY_YEAR, SalesScope::Year).unwrap();
    let invaders_1980 = entries
        .iter()
        .find(|e| e.title == "Space Invaders" && e.sales_year == Some(1980))
        .expect("1980 Space Invaders");
    assert_millions(invaders_1980.units_sold_millions, 1.318655);
    // The 1981 row has no title cell of its own; its title spans down from 1980.
    let invaders_1981 = entries
        .iter()
        .find(|e| e.title == "Space Invaders" && e.sales_year == Some(1981))
        .expect("1981 Space Invaders via rowspan");
    assert_millions(invaders_1981.units_sold_millions, 2.964137);
    let pac_man = entry(&entries, "Pac-Man");
    assert_eq!(pac_man.sales_year, Some(1982));
    assert_millions(pac_man.units_sold_millions, 7.271844);
}

#[test]
fn numbered_release_year_table_is_not_a_year_scope_table() {
    assert!(parse_entries(US_NUMBERED_SKIP, SalesScope::Year).is_err());
}

#[test]
fn numbered_table_parses_for_platform_scope_with_dash_rows_skipped() {
    let entries = parse_entries(US_NUMBERED_SKIP, SalesScope::Platform).unwrap();
    assert!(entries.iter().all(|e| e.title != "Grand Theft Auto V"));
    assert_millions(
        entry(&entries, "Pokémon Red / Blue / Yellow / Green").units_sold_millions,
        19.37,
    );
}

#[test]
fn section_rows_and_edit_links_do_not_produce_entries() {
    let html = r#"
        <table class="wikitable">
          <tr><th>Year</th><th>Top-selling title</th><th>Sales</th></tr>
          <tr><td colspan="3">1990–1999<span class="mw-editsection"><span>[</span>edit<span>]</span></span></td></tr>
          <tr><td>1990</td><td>Super Mario World<sup>[1]</sup></td><td>20,000,000<sup>[2]</sup></td></tr>
        </table>"#;
    let entries = parse_entries(html, SalesScope::Year).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].title, "Super Mario World");
    assert_eq!(entries[0].sales_year, Some(1990));
    assert_millions(entries[0].units_sold_millions, 20.0);
}

#[test]
fn page_without_a_sales_table_is_an_error() {
    let html = r#"<table class="wikitable"><tr><th>Platform</th><th>Launch</th></tr></table>"#;
    assert!(parse_entries(html, SalesScope::Platform).is_err());
}

#[test]
fn units_parsing() {
    assert_eq!(parse_units("—", false), None);
    assert_eq!(parse_units("N/a", false), None);
    assert_eq!(parse_units("2010–2019", false), None);
    assert_eq!(parse_units("1,000,000+", false), Some(1.0));
    assert_eq!(parse_units("82.90\u{a0}million[5]", false), Some(82.9));
    assert_eq!(parse_units("41 million", false), Some(41.0));
    assert_eq!(parse_units("1.5 billion", false), Some(1500.0));
    assert_eq!(parse_units("520", true), Some(520.0));
    assert_eq!(parse_units("1,318,655", false), Some(1.318655));
    assert_eq!(parse_units("1,900,000+\n\n2010–2019", false), Some(1.9));
}

#[test]
fn title_cleaning() {
    assert_eq!(
        clean_title("Super Mario 64 †").as_deref(),
        Some("Super Mario 64")
    );
    assert_eq!(clean_title("Wii Sports [a]").as_deref(), Some("Wii Sports"));
    assert_eq!(
        clean_title("  Red Dead\u{a0}Redemption 2 ").as_deref(),
        Some("Red Dead Redemption 2")
    );
    assert_eq!(clean_title("[edit]"), None);
}

#[test]
fn source_lists_are_well_formed() {
    assert_eq!(LISTS.len(), 26);
    for list in LISTS {
        assert_eq!(
            list.platform_slug.is_some(),
            list.scope == SalesScope::Platform,
            "{}",
            list.title
        );
    }
}
