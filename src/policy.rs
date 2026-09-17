// Only path-based scope selection. Source contents never participate in classification.
pub const VERSION: &str = "source-and-dependency-v2";
pub fn eligible(path: &str) -> bool {
    let mut parts = path.split('/').collect::<Vec<_>>();
    let original = parts.pop().unwrap_or("");
    let excluded = "test tests __tests__ spec specs __mocks__ mocks fixture fixtures __fixtures__ testdata testing e2e cypress __snapshots__ example examples sample samples demo demos tutorial tutorials benchmark benchmarks doc docs documentation static public assets images fonts media node_modules vendor vendors third_party third-party dist build out target coverage generated __generated__ .git .next .nuxt .cache .venv venv __pycache__ .yarn .pnpm-store";
    if parts
        .iter()
        .any(|p| excluded.split_whitespace().any(|e| e == p.to_lowercase()))
    {
        return false;
    }
    let name = original.to_lowercase();
    // Include dependency evidence for Jev; filenames do not assign a category.
    if "cargo.toml cargo.lock package.json package-lock.json npm-shrinkwrap.json yarn.lock pnpm-lock.yaml bun.lock bun.lockb deno.json deno.jsonc deno.lock go.mod go.sum go.work go.work.sum pyproject.toml poetry.lock uv.lock pipfile pipfile.lock requirements.txt setup.cfg setup.py gemfile gemfile.lock gems.rb gems.locked composer.json composer.lock pom.xml build.gradle build.gradle.kts settings.gradle settings.gradle.kts gradle.properties gradle.lockfile libs.versions.toml packages.config packages.lock.json directory.packages.props paket.dependencies paket.lock mix.exs mix.lock rebar.config rebar.lock pubspec.yaml pubspec.lock package.swift package.resolved podfile podfile.lock cartfile cartfile.resolved conanfile.txt conanfile.py conan.lock vcpkg.json vcpkg-configuration.json"
        .split_whitespace()
        .any(|n| n == name)
        || (name.starts_with("requirements-") && name.ends_with(".txt"))
        || name.rsplit_once('.').is_some_and(|(_, ext)| {
            ["csproj", "fsproj", "vbproj", "gemspec"].contains(&ext)
        })
    {
        return true;
    }
    let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
    let original_stem = original.rsplit_once('.').map_or(original, |(s, _)| s);
    if name
        .split('.')
        .any(|p| ["test", "tests", "spec", "specs", "stories", "story", "snap"].contains(&p))
        || ["test_", "test-"].iter().any(|p| stem.starts_with(p))
        || ["_test", "_tests", "-test", "_spec", "-spec"]
            .iter()
            .any(|p| stem.ends_with(p))
        || ["Test", "Tests", "Spec"]
            .iter()
            .any(|p| original_stem.ends_with(p))
    {
        return false;
    }
    if [".min.", ".bundle.", ".generated.", ".g."]
        .iter()
        .any(|p| name.contains(p))
        || [".pb.go", "_pb2.py", ".d.ts", ".d.mts", ".d.cts"]
            .iter()
            .any(|p| name.ends_with(p))
    {
        return false;
    }
    if "dockerfile containerfile makefile gnumakefile rakefile gemfile jenkinsfile vagrantfile"
        .split_whitespace()
        .any(|n| n == name)
    {
        return true;
    }
    let extensions = "js jsx mjs cjs ts tsx mts cts vue svelte astro py pyw pyx pxd rb php phtml go rs c h cc hh cpp hpp cxx hxx m mm cs fs fsx vb java kt kts scala sc swift dart ex exs erl hrl hs lhs clj cljs cljc edn lua pl pm r jl sh bash zsh fish ps1 psm1 bat cmd sql sol vy zig nim ml mli ocaml elm groovy gvy gradle tf hcl proto graphql gql wasmtext wat asm s v sv vhd vhdl f f90 f95 pas d tcl raku cr html htm ejs hbs mustache erb twig jinja jinja2 j2";
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| extensions.split_whitespace().any(|e| e == ext))
}
#[cfg(test)]
mod tests {
    #[test]
    fn scope() {
        for s in [
            "tests/x.rs",
            "src/x_test.go",
            "public/x.js",
            "a.md",
            "data.csv",
            "x.min.js",
            "tests/package.json",
            "examples/Cargo.lock",
            "node_modules/pkg/package.json",
            "data.json",
        ] {
            assert!(!super::eligible(s), "{s}");
        }
        for s in [
            "src/main.rs",
            "src/contest.py",
            "Makefile",
            "a.html",
            "Cargo.toml",
            "Cargo.lock",
            "frontend/package.json",
            "pnpm-lock.yaml",
            "requirements.txt",
            "requirements-dev.txt",
            "go.sum",
            "project.csproj",
        ] {
            assert!(super::eligible(s), "{s}");
        }
    }
}
