//! Focused tests for `library/std/json`.
//!
//! These tests drive the C ABI directly and compile only the runtime files
//! this package uses. Release e2e fixtures cover the package surface through
//! the Riven compiler.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}_{}", std::process::id(), n)
}

fn json_runtime_sources(root: &Path) -> Vec<PathBuf> {
    [
        "library/std/core/runtime/alloc.c",
        "library/std/string/runtime/string.c",
        "library/std/array/runtime/vec.c",
        "library/std/hash/runtime/hash.c",
        "library/std/json/runtime/json.c",
    ]
    .into_iter()
    .map(|rel| root.join(rel))
    .collect()
}

fn compile_objects(extra_flags: &[&str]) -> Vec<PathBuf> {
    let root = workspace_root();
    let mut objects = Vec::new();
    for src in json_runtime_sources(&root) {
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("json");
        let obj = std::env::temp_dir().join(format!("riven_json_{stem}_{}.o", unique_suffix()));
        let mut cmd = Command::new("cc");
        cmd.arg("-c").arg(&src).arg("-o").arg(&obj);
        for flag in extra_flags {
            cmd.arg(flag);
        }
        let status = cmd.status().expect("invoke cc");
        assert!(
            status.success(),
            "failed to compile {} with flags {:?}",
            src.display(),
            extra_flags
        );
        objects.push(obj);
    }
    objects
}

fn compile_harness(name: &str, source: &str) -> PathBuf {
    let objects = compile_objects(&["-O2"]);
    let suffix = unique_suffix();
    let harness_c = std::env::temp_dir().join(format!("riven_json_{name}_{suffix}.c"));
    let harness_bin = std::env::temp_dir().join(format!("riven_json_{name}_{suffix}"));
    std::fs::write(&harness_c, source).expect("write harness");

    let mut cmd = Command::new("cc");
    cmd.arg(&harness_c);
    for obj in &objects {
        cmd.arg(obj);
    }
    cmd.arg("-o").arg(&harness_bin);
    let status = cmd.status().expect("invoke cc for harness");

    let _ = std::fs::remove_file(&harness_c);
    for obj in &objects {
        let _ = std::fs::remove_file(obj);
    }
    assert!(status.success(), "failed to compile harness {name}");
    harness_bin
}

fn run_harness(name: &str, source: &str) {
    let harness = compile_harness(name, source);
    let output = Command::new(&harness).output().expect("run harness");
    let _ = std::fs::remove_file(&harness);
    assert!(
        output.status.success(),
        "harness {name} failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn json_runtime_compiles_with_strict_warnings() {
    let objects = compile_objects(&["-O2", "-Wall", "-Wextra", "-Werror"]);
    for obj in &objects {
        let _ = std::fs::remove_file(obj);
    }
}

#[test]
fn json_parse_accepts_comments_and_trailing_commas() {
    run_harness(
        "relaxed_parse",
        r#"
#include <stdint.h>
#include <string.h>

void *riven_json_parse(const char *input);
void *riven_json_parse_strict(const char *input);
void *riven_json_object_get(void *value, const char *key);
void *riven_json_array_len(void *value);
void *riven_json_as_int(void *value);
void *riven_json_as_bool(void *value);

static int tag(void *box) { return *(int32_t *)box; }
static int64_t payload(void *box) { return ((int64_t *)box)[1]; }

int main(void) {
    const char *src =
        "{\n"
        "  // leading field\n"
        "  \"answer\": 42,\n"
        "  /* array field */\n"
        "  \"flags\": [true,],\n"
        "}\n";

    void *result = riven_json_parse(src);
    if (tag(result) != 0) return 1;
    void *root = (void *)payload(result);

    void *answer_opt = riven_json_object_get(root, "answer");
    if (tag(answer_opt) != 1) return 2;
    void *answer_json = (void *)payload(answer_opt);
    void *answer_int = riven_json_as_int(answer_json);
    if (tag(answer_int) != 1 || payload(answer_int) != 42) return 3;

    void *flags_opt = riven_json_object_get(root, "flags");
    if (tag(flags_opt) != 1) return 4;
    void *flags_len = riven_json_array_len((void *)payload(flags_opt));
    if (tag(flags_len) != 1 || payload(flags_len) != 1) return 5;

    if (tag(riven_json_parse_strict(src)) != 1) return 6;
    return 0;
}
"#,
    );
}

#[test]
fn json_parse_strict_rejects_comments_and_trailing_commas() {
    run_harness(
        "strict_rejects_relaxed_syntax",
        r#"
#include <stdint.h>

void *riven_json_parse_strict(const char *input);

static int tag(void *box) { return *(int32_t *)box; }

int main(void) {
    if (tag(riven_json_parse_strict("// comment\n{\"a\": 1}")) != 1) return 1;
    if (tag(riven_json_parse_strict("{\"a\": 1,}")) != 1) return 2;
    if (tag(riven_json_parse_strict("[1, 2,]")) != 1) return 3;
    if (tag(riven_json_parse_strict("{\"a\": [1, 2]}")) != 0) return 4;
    return 0;
}
"#,
    );
}

#[test]
fn json_builders_marshal_values_into_json_tree() {
    run_harness(
        "builder_marshalling",
        r#"
#include <stdint.h>
#include <string.h>

typedef struct RivenVec RivenVec;
typedef struct RivenHash RivenHash;

RivenVec *riven_vec_new(void);
void riven_vec_push(RivenVec *v, int64_t item);
RivenHash *riven_hash_new(void);
void riven_hash_insert(RivenHash *h, int64_t key, int64_t value);
char *riven_string_from(const char *s);

void *riven_json_make_bool(int8_t value);
void *riven_json_make_int(int64_t value);
void *riven_json_make_string(const char *value);
void *riven_json_make_array(RivenVec *items);
void *riven_json_make_object(RivenHash *fields);
void *riven_json_stringify(void *value);
void *riven_json_object_get(void *value, const char *key);
void *riven_json_as_bool(void *value);
void *riven_json_as_string(void *value);

static int tag(void *box) { return *(int32_t *)box; }
static int64_t payload(void *box) { return ((int64_t *)box)[1]; }

int main(void) {
    RivenVec *items = riven_vec_new();
    riven_vec_push(items, (int64_t)riven_json_make_int(7));
    riven_vec_push(items, (int64_t)riven_json_make_bool(1));

    RivenHash *fields = riven_hash_new();
    riven_hash_insert(fields, (int64_t)riven_string_from("name"), (int64_t)riven_json_make_string("riven"));
    riven_hash_insert(fields, (int64_t)riven_string_from("items"), (int64_t)riven_json_make_array(items));
    riven_hash_insert(fields, (int64_t)riven_string_from("enabled"), (int64_t)riven_json_make_bool(1));

    void *root = riven_json_make_object(fields);
    void *enabled_opt = riven_json_object_get(root, "enabled");
    if (tag(enabled_opt) != 1) return 1;
    void *enabled_bool = riven_json_as_bool((void *)payload(enabled_opt));
    if (tag(enabled_bool) != 1 || payload(enabled_bool) != 1) return 2;

    void *name_opt = riven_json_object_get(root, "name");
    if (tag(name_opt) != 1) return 3;
    void *name_string = riven_json_as_string((void *)payload(name_opt));
    if (tag(name_string) != 1 || strcmp((const char *)payload(name_string), "riven") != 0) return 4;

    void *rendered = riven_json_stringify(root);
    if (tag(rendered) != 0) return 5;
    const char *json = (const char *)payload(rendered);
    if (!strstr(json, "\"name\":\"riven\"")) return 6;
    if (!strstr(json, "\"enabled\":true")) return 7;
    if (!strstr(json, "\"items\":[7,true]")) return 8;
    return 0;
}
"#,
    );
}
