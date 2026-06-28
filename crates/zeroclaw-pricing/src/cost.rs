use crate::schema::ModelPrice;

const PER_MILLION: f64 = 1_000_000.0;

pub fn cost(
    price: &ModelPrice,
    tokens_in: u64,
    tokens_out: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
) -> f64 {
    let input = price.input.unwrap_or(0.0);
    let output = price.output.unwrap_or(0.0);
    let cache_read = price.cache_read.unwrap_or(0.0);
    let cache_write = price.cache_write.unwrap_or(0.0);

    (tokens_in as f64 * input
        + tokens_out as f64 * output
        + cache_read_tokens as f64 * cache_read
        + cache_write_tokens as f64 * cache_write)
        / PER_MILLION
}
