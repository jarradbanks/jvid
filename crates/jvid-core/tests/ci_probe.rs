#[test]
fn ci_probe() {
    assert_eq!(probe(), 1);
}

fn probe() -> i32 {
    return 1;
}
