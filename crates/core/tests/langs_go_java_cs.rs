//! Go, Java and C# tree-sitter packs: definitions, exports, tests, imports, dead code.
use plumbgraph_core::ops::{run_dead_code, DeadCodeParams, ScipOpts, Target};
use std::path::Path;

fn write(root: &Path, rel: &str, c: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, c).unwrap();
}

fn dead(root: &Path) -> Vec<String> {
    let r = run_dead_code(
        &Target {
            root: root.to_path_buf(),
            db: None,
        },
        &DeadCodeParams {
            min_confidence: 0.0,
            scip: ScipOpts::disabled(),
            ..Default::default()
        },
    )
    .unwrap();
    let mut v: Vec<String> = r
        .findings
        .iter()
        .filter_map(|f| f.symbol.clone())
        .map(|q| q.rsplit('.').next().unwrap_or(&q).to_string())
        .collect();
    v.sort();
    v
}

#[test]
fn go_dead_code_exports_tests_and_imports() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    write(r, "go.mod", "module example.com/app\n\ngo 1.22\n");
    write(
        r,
        "main.go",
        "package main\n\nimport (\n\t\"fmt\"\n\t\"example.com/app/util\"\n)\n\nfunc main() {\n\tfmt.Println(util.Helper())\n\tused()\n\ts := Server{}\n\ts.Start()\n}\n\nfunc used() {}\n\nfunc unusedPrivate() {}\n\ntype deadType struct{}\n\ntype Server struct{}\n\nfunc (s *Server) Start() { s.stop() }\n\nfunc (s *Server) stop() {}\n\nfunc (s *Server) neverCalled() {}\n",
    );
    write(
        r,
        "util/util.go",
        "package util\n\nfunc Helper() string { return inner() }\n\nfunc inner() string { return \"x\" }\n\nfunc orphanInner() {}\n",
    );
    write(
        r,
        "util/util_test.go",
        "package util\n\nimport \"testing\"\n\nfunc TestHelper(t *testing.T) { _ = Helper() }\n",
    );
    let d = dead(r);
    for want in ["unusedPrivate", "deadType", "neverCalled", "orphanInner"] {
        assert!(d.contains(&want.to_string()), "missing {want}: {d:?}");
    }
    for not in [
        "main",
        "used",
        "Helper",
        "inner",
        "stop",
        "TestHelper",
        "init",
    ] {
        assert!(!d.contains(&not.to_string()), "false positive {not}: {d:?}");
    }
}

#[test]
fn java_dead_code_annotations_and_tests() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    write(
        r,
        "src/main/java/com/acme/App.java",
        "package com.acme;\n\nimport com.acme.util.Helper;\n\npublic class App {\n    public static void main(String[] args) {\n        System.out.println(Helper.help());\n        used();\n    }\n    private static void used() {}\n    private static void unusedPrivate() {}\n    @Override\n    public String toString() { return \"x\"; }\n}\n\nclass DeadClass {\n    void nothing() {}\n}\n",
    );
    write(
        r,
        "src/main/java/com/acme/util/Helper.java",
        "package com.acme.util;\n\npublic class Helper {\n    public static String help() { return inner(); }\n    private static String inner() { return \"x\"; }\n    private static void orphan() {}\n}\n",
    );
    write(
        r,
        "src/test/java/com/acme/AppTest.java",
        "package com.acme;\n\nimport org.junit.jupiter.api.Test;\n\nclass AppTest {\n    @Test\n    void worksFine() { App.main(new String[0]); }\n}\n",
    );
    let d = dead(r);
    for want in ["unusedPrivate", "DeadClass", "orphan"] {
        assert!(d.contains(&want.to_string()), "missing {want}: {d:?}");
    }
    for not in [
        "main",
        "used",
        "help",
        "inner",
        "toString",
        "worksFine",
        "App",
    ] {
        assert!(!d.contains(&not.to_string()), "false positive {not}: {d:?}");
    }
}

#[test]
fn csharp_dead_code_attributes_and_tests() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    write(
        r,
        "src/Program.cs",
        "using System;\nusing Acme.Util;\n\nnamespace Acme\n{\n    public class Program\n    {\n        public static void Main(string[] args)\n        {\n            Console.WriteLine(Helper.Help());\n            Used();\n        }\n        private static void Used() { }\n        private static void UnusedPrivate() { }\n        public override string ToString() { return \"x\"; }\n    }\n\n    internal class DeadClass\n    {\n        public void Nothing() { }\n    }\n}\n",
    );
    write(
        r,
        "src/Util/Helper.cs",
        "namespace Acme.Util\n{\n    public static class Helper\n    {\n        public static string Help() { return Inner(); }\n        private static string Inner() { return \"x\"; }\n        private static void Orphan() { }\n    }\n}\n",
    );
    write(
        r,
        "tests/ProgramTests.cs",
        "using Xunit;\n\nnamespace Acme.Tests\n{\n    public class ProgramTests\n    {\n        [Fact]\n        public void WorksFine() { Acme.Program.Main(new string[0]); }\n    }\n}\n",
    );
    let d = dead(r);
    for want in ["UnusedPrivate", "DeadClass", "Orphan"] {
        assert!(d.contains(&want.to_string()), "missing {want}: {d:?}");
    }
    for not in [
        "Main",
        "Used",
        "Help",
        "Inner",
        "ToString",
        "WorksFine",
        "Program",
    ] {
        assert!(!d.contains(&not.to_string()), "false positive {not}: {d:?}");
    }
}
