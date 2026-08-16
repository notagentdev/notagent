pub fn compute_checksum(data: &[u8]) -> u64 {
    data.iter().map(|b| *b as u64).sum()
}
