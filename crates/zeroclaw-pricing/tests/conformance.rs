use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use zeroclaw_pricing::{
    ModelPrice, Source, canonicalize, cost, normalize_litellm, normalize_openrouter,
};

#[derive(Debug, Deserialize)]
struct NormalizeCase {
    model_id_raw: String,
    raw: Value,
    expected: ModelPrice,
}

#[derive(Debug, Deserialize)]
struct CanonCase {
    raw: String,
    expected: CanonExpected,
}

#[derive(Debug, Deserialize)]
struct CanonExpected {
    provider: Option<String>,
    model_id: String,
}

#[derive(Debug, Deserialize)]
struct CostCase {
    price: CostPrice,
    tokens_in: u64,
    tokens_out: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    expected_cost: f64,
}

#[derive(Debug, Deserialize)]
struct CostPrice {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

#[test]
fn litellm_vectors_match_reference() {
    for fixture in fixtures("tests/conformance/litellm") {
        let case: NormalizeCase = read_json(&fixture);
        let actual = normalize_litellm(&case.model_id_raw, &case.raw);
        assert_model_price_close(&actual, &case.expected, &fixture);
    }
}

#[test]
fn openrouter_vectors_match_reference() {
    for fixture in fixtures("tests/conformance/openrouter") {
        let case: NormalizeCase = read_json(&fixture);
        let actual = normalize_openrouter(&case.model_id_raw, &case.raw);
        assert_model_price_close(&actual, &case.expected, &fixture);
    }
}

#[test]
fn canonicalization_vectors_match_reference() {
    let cases: Vec<CanonCase> = read_json(Path::new("tests/conformance/canon/cases.json"));
    for case in cases {
        let (provider, model_id) = canonicalize(&case.raw);
        assert_eq!(
            provider, case.expected.provider,
            "provider for {}",
            case.raw
        );
        assert_eq!(
            model_id, case.expected.model_id,
            "model_id for {}",
            case.raw
        );
    }
}

#[test]
fn cost_vectors_match_reference() {
    let cases: Vec<CostCase> = read_json(Path::new("tests/conformance/cost/cases.json"));
    for case in cases {
        let price = ModelPrice {
            model_id: "cost-vector".to_string(),
            input: case.price.input,
            output: case.price.output,
            cache_read: case.price.cache_read,
            cache_write: case.price.cache_write,
            source: Source::Catalog,
        };
        let actual = cost(
            &price,
            case.tokens_in,
            case.tokens_out,
            case.cache_read_tokens,
            case.cache_write_tokens,
        );
        assert!(
            (actual - case.expected_cost).abs() <= 1e-9,
            "cost mismatch: actual={actual} expected={}",
            case.expected_cost
        );
    }
}

fn fixtures(dir: &str) -> Vec<std::path::PathBuf> {
    let mut entries = fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("read_dir {dir}: {err}"))
        .map(|entry| entry.expect("fixture dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> T {
    let raw =
        fs::read_to_string(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|err| panic!("parse {}: {err}", path.display()))
}

fn assert_model_price_close(actual: &ModelPrice, expected: &ModelPrice, fixture: &Path) {
    assert_eq!(actual.model_id, expected.model_id, "{}", fixture.display());
    assert_price(actual.input, expected.input, "input", fixture);
    assert_price(actual.output, expected.output, "output", fixture);
    assert_price(
        actual.cache_read,
        expected.cache_read,
        "cache_read",
        fixture,
    );
    assert_price(
        actual.cache_write,
        expected.cache_write,
        "cache_write",
        fixture,
    );
    assert_eq!(actual.source, expected.source, "{}", fixture.display());
}

fn assert_price(actual: Option<f64>, expected: Option<f64>, label: &str, fixture: &Path) {
    match (actual, expected) {
        (Some(actual), Some(expected)) => assert!(
            (actual - expected).abs() <= 1e-9,
            "{} {label}: actual={actual} expected={expected}",
            fixture.display()
        ),
        (None, None) => {}
        _ => panic!(
            "{} {label}: actual={actual:?} expected={expected:?}",
            fixture.display()
        ),
    }
}
