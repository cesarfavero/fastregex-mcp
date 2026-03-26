use xxhash_rust::xxh3::xxh3_64;

#[inline]
pub fn hash_gram(bytes: &[u8]) -> u64 {
    xxh3_64(bytes)
}

#[inline]
pub fn hash_repo_id(input: &str) -> String {
    format!("{:016x}", xxh3_64(input.as_bytes()))
}
