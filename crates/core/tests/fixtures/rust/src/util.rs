pub fn used_fn() {
    helper();
}

fn helper() {}

pub fn pub_unused() {}

pub struct S;

impl S {
    pub fn new() -> S {
        S
    }
}

impl std::fmt::Display for S {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "S")
    }
}
