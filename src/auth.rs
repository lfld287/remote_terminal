pub fn verify_token(provided: &str, expected: &str) -> bool {
    if expected.is_empty() {
        return true;
    }
    const_eq(provided, expected)
}

fn const_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        result |= x ^ y;
    }
    result == 0
}
