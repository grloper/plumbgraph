mod util;
use util::used_fn;

fn main() {
    used_fn();
}

fn dead_private() {}

struct DeadStruct;

fn test_only() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t() {
        test_only();
    }
}
