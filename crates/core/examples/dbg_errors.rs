use plumbgraph_langs::PackSet;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let set = PackSet::builtin().unwrap();
    let src = std::fs::read_to_string(&a[1]).unwrap();
    let pack = set.for_path(std::path::Path::new(&a[1])).unwrap();
    let mut p = tree_sitter::Parser::new();
    p.set_language(&pack.language().unwrap()).unwrap();
    let t = p.parse(&src, None).unwrap();
    let mut stack = vec![t.root_node()];
    while let Some(n) = stack.pop() {
        if n.is_error() || n.is_missing() {
            let l = n.start_position().row;
            println!(
                "{} line {}: {}",
                if n.is_missing() { "MISSING" } else { "ERROR" },
                l + 1,
                src.lines().nth(l).unwrap_or("").trim()
            );
        }
        let mut c = n.walk();
        for ch in n.children(&mut c) {
            stack.push(ch);
        }
    }
}
