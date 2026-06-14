//! Object file emission and native linking.

use std::path::{Path, PathBuf};
use std::process::Command;

fn linker_args(sanitize: bool, extra_link_flags: &[String]) -> Vec<String> {
    // B3 of `docs/specs/system/zero_rust_stdlib_classes.spec.md`:
    // `-lc / -lm / -lpthread` are now declared in per-package
    // `Ruxen.toml` `[system_libs]` tables (`library/std/core/Ruxen.toml`,
    // `library/std/sync/Ruxen.toml`). `codegen::compile_with_options`
    // aggregates them into `extra_link_flags` BEFORE calling this
    // function, so they arrive here in the caller-supplied slice
    // alongside FFI-attribute flags from `@[link("...")]`. Only
    // sanitizer + macOS framework flags remain compiler-resident
    // because they are not package-specific.
    let mut args: Vec<String> = Vec::new();
    // Phase 2 #06.5 T8: std::rand on macOS uses SecRandomCopyBytes from
    // the Security framework. The runtime's `#include <Security/...>`
    // covers the header side; the linker still needs the framework
    // flag to resolve the symbol at link time.
    if cfg!(target_os = "macos") {
        args.push("-framework".to_string());
        args.push("Security".to_string());
    }
    if sanitize {
        args.push("-fsanitize=address,undefined".to_string());
    }
    args.extend(extra_link_flags.iter().cloned());
    args
}

/// Apply the vendored-PCRE2 include path + width defines to a `cc -c`
/// invocation when `runtime_c_path` is a PCRE2 vendor source.
///
/// PCRE2's `.c` files `#include "config.h"`, `"pcre2.h"`,
/// `"pcre2_internal.h"`, etc. via bare filenames, so the vendor dir goes on
/// the include path. `HAVE_CONFIG_H` selects our hand-authored config.h;
/// `PCRE2_CODE_UNIT_WIDTH=8` selects the 8-bit build (matches the REPL JIT
/// build in `src/ruxen_repl/build.rs`). The detection key is a path component
/// literally named `pcre2` — covers the vendor tree, never the user-authored
/// `library/std/regex/runtime/regex.c`. Shared by the host and cross runtime
/// compile paths so they stay in lockstep.
fn apply_pcre2_flags(cmd: &mut Command, runtime_c_path: &Path) {
    if runtime_c_path
        .components()
        .any(|c| c.as_os_str() == "pcre2")
    {
        if let Some(parent) = runtime_c_path.parent() {
            cmd.arg("-I").arg(parent);
        }
        cmd.arg("-DHAVE_CONFIG_H=1")
            .arg("-DPCRE2_CODE_UNIT_WIDTH=8")
            .arg("-Wno-sign-compare")
            .arg("-Wno-unused-parameter")
            .arg("-Wno-implicit-fallthrough");
    }
}

/// Compile a single C runtime source to an object file.
///
/// When `sanitize` is true, the file is compiled with AddressSanitizer
/// and UndefinedBehaviorSanitizer instrumentation. Kept as the primitive
/// shared by both the singleton path (legacy unity-build callers via
/// [`compile_runtime`]) and the multi-source path
/// ([`compile_runtime_sources`]) introduced in #06.95 Phase B-2.
pub fn compile_runtime(runtime_c_path: &Path, sanitize: bool) -> Result<PathBuf, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stem = runtime_c_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("runtime");
    let runtime_o = std::env::temp_dir().join(format!(
        "ruxen_{}_{}_{}.o",
        stem,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    ));

    let mut cmd = Command::new("cc");
    cmd.arg("-c").arg(runtime_c_path).arg("-o").arg(&runtime_o);

    // Pin the macOS deployment target to match libruxenrt.a's prebuilt
    // objects (ruxen_repl/build.rs), the Cranelift LC_BUILD_VERSION, and
    // the link — all 11.0 — so ld never reports a version skew.
    #[cfg(target_os = "macos")]
    cmd.arg("-mmacosx-version-min=11.0");

    if sanitize {
        cmd.arg("-fsanitize=address,undefined")
            .arg("-g")
            .arg("-fno-omit-frame-pointer");
    } else {
        cmd.arg("-O2");
    }

    apply_pcre2_flags(&mut cmd, runtime_c_path);

    let status = cmd.status().map_err(|e| {
        format!(
            "Failed to invoke cc for {}: {}",
            runtime_c_path.display(),
            e
        )
    })?;

    if !status.success() {
        return Err(format!("Failed to compile {}", runtime_c_path.display()));
    }

    Ok(runtime_o)
}

/// Compile every per-package C runtime source to its own object file.
///
/// After #06.95 Phase B-2, each stdlib package's `.c` files are
/// standalone translation units (they `#include` the shared
/// `library/std/core/runtime/runtime.h` for cross-package type and
/// function declarations). This walks the source list, compiles each
/// to a uniquely named `.o`, and returns the full set for the linker.
///
/// On failure, any objects already produced are best-effort removed.
pub fn compile_runtime_sources(
    sources: &[PathBuf],
    sanitize: bool,
) -> Result<Vec<PathBuf>, String> {
    let mut objects: Vec<PathBuf> = Vec::with_capacity(sources.len());
    for src in sources {
        match compile_runtime(src, sanitize) {
            Ok(o) => objects.push(o),
            Err(e) => {
                for partial in &objects {
                    let _ = std::fs::remove_file(partial);
                }
                return Err(e);
            }
        }
    }
    Ok(objects)
}

/// Write object bytes to a file and link with the runtime into an executable.
///
/// When `sanitize` is true, the linker is invoked with sanitizer flags so that
/// the sanitizer runtime is linked into the final binary.
///
/// `extra_link_flags` provides additional linker flags (e.g., `-lfoo` from
/// `@[link("foo")]` FFI attributes).
pub fn emit_executable(
    object_bytes: &[u8],
    runtime_objects: &[PathBuf],
    output_path: &str,
    sanitize: bool,
    extra_link_flags: &[String],
) -> Result<(), String> {
    let obj_path = format!("{}.o", output_path);

    std::fs::write(&obj_path, object_bytes)
        .map_err(|e| format!("Failed to write object file: {}", e))?;

    let mut cmd = Command::new("cc");
    cmd.arg(&obj_path);
    for runtime_o in runtime_objects {
        cmd.arg(runtime_o);
    }
    cmd.arg("-o").arg(output_path);
    // Match the deployment target of every input object (11.0) so the link
    // target isn't the lower OS-major default, which makes ld flag the
    // prebuilt archive as "built for a newer macOS version".
    #[cfg(target_os = "macos")]
    cmd.arg("-mmacosx-version-min=11.0");

    for arg in linker_args(sanitize, extra_link_flags) {
        cmd.arg(arg);
    }

    let status = cmd
        .status()
        .map_err(|e| format!("Failed to invoke linker: {}", e))?;

    if !status.success() {
        return Err(format!("Linking failed for '{}'", output_path));
    }

    let _ = std::fs::remove_file(&obj_path);
    for runtime_o in runtime_objects {
        let _ = std::fs::remove_file(runtime_o);
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────
// Cross-compilation linking (tier 4.02)
// ─────────────────────────────────────────────────────────────────────────

use crate::codegen::target::{LinkerSpec, ResolvedTarget};

/// Compile one runtime `.c` for a cross target using a target-aware `cc`.
///
/// `target_args` are the leading flags from the resolved [`LinkerSpec`]
/// (e.g. `-arch x86_64` for a Darwin cross). The same `cc` driver that links
/// also compiles the runtime, so the runtime object matches the target ABI.
/// Only used on the *local* cross path (Darwin→Darwin); the container path
/// compiles the runtime inside the container instead (see
/// [`emit_executable_in_container`]).
pub fn compile_runtime_for_target(
    runtime_c_path: &Path,
    target: &ResolvedTarget,
    target_args: &[String],
) -> Result<PathBuf, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stem = runtime_c_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("runtime");
    let runtime_o = std::env::temp_dir().join(format!(
        "ruxen_{}_{}_{}_{}.o",
        stem,
        target.canonical().replace('-', "_"),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    ));

    let mut cmd = Command::new("cc");
    for a in target_args {
        cmd.arg(a);
    }
    cmd.arg("-c").arg(runtime_c_path).arg("-o").arg(&runtime_o);
    // Darwin cross still pins the deployment floor so ld doesn't complain.
    if target.is_darwin() {
        cmd.arg("-mmacosx-version-min=11.0");
    }
    cmd.arg("-O2");
    // Vendored PCRE2 needs the same include path + width defines the host
    // `compile_runtime` applies; without them `pcre2_internal.h` aborts the
    // compile ("PCRE2_CODE_UNIT_WIDTH must be defined").
    apply_pcre2_flags(&mut cmd, runtime_c_path);

    let status = cmd.status().map_err(|e| {
        format!(
            "Failed to invoke cc for {}: {}",
            runtime_c_path.display(),
            e
        )
    })?;
    if !status.success() {
        return Err(format!(
            "Failed to compile runtime {} for target {}",
            runtime_c_path.display(),
            target.canonical()
        ));
    }
    Ok(runtime_o)
}

/// Link a cross-compiled executable using a resolved [`LinkerSpec`].
///
/// Dispatches on `spec.needs_container`:
/// - local link → spawn `spec.program` with `spec.target_args` (Darwin cross
///   via `cc -arch`, or a host-arch Linux `cc`, or an on-PATH cross gcc).
/// - container link → the two-stage Docker flow ([`emit_executable_in_container`]).
///
/// `runtime_sources` are the stdlib `.c` paths; on the local path they must
/// already be compiled to `runtime_objects`. On the container path they are
/// compiled *inside* the container (the host has no target cc), so the caller
/// passes the source paths and an empty `runtime_objects`.
#[allow(clippy::too_many_arguments)]
pub fn emit_executable_for_target(
    object_bytes: &[u8],
    runtime_objects: &[PathBuf],
    runtime_sources: &[PathBuf],
    output_path: &str,
    target: &ResolvedTarget,
    spec: &LinkerSpec,
    extra_link_flags: &[String],
) -> Result<(), String> {
    if spec.needs_container {
        return emit_executable_in_container(
            object_bytes,
            runtime_sources,
            output_path,
            target,
            extra_link_flags,
        );
    }

    // Local cross link (Darwin→Darwin, native Linux, or on-PATH cross gcc).
    let obj_path = format!("{}.o", output_path);
    std::fs::write(&obj_path, object_bytes)
        .map_err(|e| format!("Failed to write object file: {}", e))?;

    let mut cmd = Command::new(&spec.program);
    for a in &spec.target_args {
        cmd.arg(a);
    }
    cmd.arg(&obj_path);
    for runtime_o in runtime_objects {
        cmd.arg(runtime_o);
    }
    cmd.arg("-o").arg(output_path);
    if target.is_darwin() {
        cmd.arg("-mmacosx-version-min=11.0");
        // std::rand uses SecRandomCopyBytes; the Security framework must be
        // linked. (Cross-platform Linux targets don't link this.)
        cmd.arg("-framework").arg("Security");
    }
    for flag in extra_link_flags {
        cmd.arg(flag);
    }

    let status = cmd.status().map_err(|e| {
        format!(
            "Failed to invoke cross linker '{}' for {}: {}",
            spec.program,
            target.canonical(),
            e
        )
    })?;
    if !status.success() {
        return Err(format!(
            "Cross-linking failed for '{}' (target {})",
            output_path,
            target.canonical()
        ));
    }

    let _ = std::fs::remove_file(&obj_path);
    for runtime_o in runtime_objects {
        let _ = std::fs::remove_file(runtime_o);
    }
    Ok(())
}

/// Tier 4.03 (WASM): link a WebAssembly object into a final `.wasm` module
/// with `wasm-ld`.
///
/// A `wasm32-unknown-unknown` module is a *reactor* (spec §5.3): no `_start`,
/// no libc, no C runtime — its exports are the host-callable API. The LLVM
/// backend already emitted the object with `export_name` attributes on every
/// `program.wasm_exports` entry, so `--export-dynamic` keeps them live through
/// `wasm-ld`'s default `--gc-sections`. `--allow-undefined` tolerates host
/// imports (none in the math-export v1 path, but harmless and forward-looking).
///
/// `wasm-ld` discovery order: `RUXEN_WASM_LD` env override → the LLVM-18 prefix
/// (`/opt/homebrew/opt/llvm@18/bin/wasm-ld`, where the cross-compile work
/// already assumes LLVM 18 lives) → bare `wasm-ld` on `PATH`. A missing linker
/// errors with an actionable install hint rather than a raw spawn failure.
fn find_wasm_ld() -> Result<String, String> {
    if let Some(p) = std::env::var_os("RUXEN_WASM_LD") {
        let s = p.to_string_lossy().to_string();
        if Path::new(&s).is_file() {
            return Ok(s);
        }
    }
    let prefixed = "/opt/homebrew/opt/llvm@18/bin/wasm-ld";
    if Path::new(prefixed).is_file() {
        return Ok(prefixed.to_string());
    }
    // Fall back to PATH (Linux distros ship `wasm-ld` via the `lld` package;
    // rustup's `rust-lld` is `wasm-ld` under the hood).
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let cand = dir.join("wasm-ld");
            if cand.is_file() {
                return Ok(cand.to_string_lossy().to_string());
            }
        }
    }
    Err(
        "wasm-ld not found. Install LLVM's lld (`brew install llvm@18` or \
         `apt install lld`) or set RUXEN_WASM_LD to its path."
            .to_string(),
    )
}

/// Link a WebAssembly object (plus any runtime objects) into a `.wasm` module.
/// See [`find_wasm_ld`].
///
/// `runtime_objects` are the compiled heap-core runtime + allocator/libc shim
/// objects (tier 4.09). With them present, `wasm-ld` resolves `ruxen_vec_*`,
/// `ruxen_alloc`, `malloc`, … internally instead of leaving them as host imports.
/// `--allow-undefined` is retained so any genuinely host-supplied symbol (future
/// `wasm_import` work) still links.
pub fn emit_wasm_module(
    object_bytes: &[u8],
    runtime_objects: &[PathBuf],
    output_path: &str,
    target: &ResolvedTarget,
) -> Result<(), String> {
    let wasm_ld = find_wasm_ld()?;
    let obj_path = format!("{}.o", output_path);
    std::fs::write(&obj_path, object_bytes)
        .map_err(|e| format!("Failed to write wasm object file: {}", e))?;

    let mut cmd = Command::new(&wasm_ld);
    cmd.arg("--no-entry")
        .arg("--export-dynamic")
        .arg("--allow-undefined")
        .arg(&obj_path);
    for ro in runtime_objects {
        cmd.arg(ro);
    }
    cmd.arg("-o").arg(output_path);

    let status = cmd.status().map_err(|e| {
        format!(
            "Failed to invoke wasm-ld '{}' for {}: {}",
            wasm_ld,
            target.canonical(),
            e
        )
    })?;
    if !status.success() {
        return Err(format!(
            "wasm-ld failed for '{}' (target {})",
            output_path,
            target.canonical()
        ));
    }
    // Clean temp objects only after a successful link (a failed link leaves the
    // inputs in place for inspection / retry — matches `emit_executable`).
    let _ = std::fs::remove_file(&obj_path);
    for ro in runtime_objects {
        let _ = std::fs::remove_file(ro);
    }
    Ok(())
}

/// The bundled wasm runtime shim (tier 4.09): a freestanding allocator + the few
/// libc functions the heap-core runtime needs, for `wasm32-unknown-unknown` (which
/// has no libc). Embedded as a string so it travels with the compiler (no source
/// tree needed) and is written to a temp `.c` and compiled on demand.
///
/// The allocator is a **bump allocator** over `__builtin_wasm_memory_grow` with an
/// 8-byte size header (so `realloc` can size-copy). `free` does not reclaim — a
/// free-list upgrade is a tracked follow-up; correctness (not yet peak memory) is
/// the bar for the first heap milestone. `mem*`/`qsort` are simple, correct
/// implementations; the shim is compiled with `-fno-builtin` so these definitions
/// don't get pattern-matched back into self-recursive calls.
pub const WASM_RT_C: &str = r#"/* Ruxen wasm32 runtime shim — bundled allocator + minimal libc.
 * Generated/owned by codegen/object.rs (tier 4.09). Compiled with -fno-builtin. */
#include <stdint.h>
#include <stddef.h>
#include <stdarg.h>

/* Prototypes so the allocator can call mem* before their definitions below. */
void *memcpy(void *, const void *, size_t);
void *memmove(void *, const void *, size_t);
void *memset(void *, int, size_t);
int memcmp(const void *, const void *, size_t);

#define RX_WASM_PAGE 65536u

extern unsigned char __heap_base; /* wasm-ld: first byte past static data */

static uintptr_t rx_brk;
static uintptr_t rx_end;
static int rx_init_done;

static void rx_heap_init(void) {
    if (!rx_init_done) {
        rx_brk = (uintptr_t)&__heap_base;
        rx_end = (uintptr_t)__builtin_wasm_memory_size(0) * RX_WASM_PAGE;
        rx_init_done = 1;
    }
}

static uintptr_t rx_bump(size_t total) { /* returns 16-aligned base, or 0 on OOM */
    uintptr_t p = (rx_brk + 15u) & ~(uintptr_t)15u;
    uintptr_t nb = p + total;
    if (nb > rx_end) {
        size_t need = nb - rx_end;
        size_t pages = (need + RX_WASM_PAGE - 1) / RX_WASM_PAGE;
        size_t prev = __builtin_wasm_memory_grow(0, pages);
        if (prev == (size_t)-1) return 0;
        rx_end += pages * RX_WASM_PAGE;
    }
    rx_brk = nb;
    return p;
}

void *malloc(size_t n) {
    rx_heap_init();
    size_t payload = (n + 15u) & ~(size_t)15u;
    if (payload == 0) payload = 16;
    uintptr_t base = rx_bump(payload + 16); /* 16-byte header keeps payload aligned */
    if (!base) return (void *)0;
    *(size_t *)base = payload; /* store the rounded size: realloc copies the full live payload */
    return (void *)(base + 16);
}

void free(void *p) { (void)p; /* bump allocator: no reclaim (free-list upgrade pending) */ }

void *calloc(size_t nm, size_t sz) {
    size_t tot = nm * sz;
    if (sz != 0 && tot / sz != nm) return (void *)0; /* overflow */
    void *p = malloc(tot);
    if (p) memset(p, 0, tot);
    return p;
}

void *realloc(void *p, size_t n) {
    if (!p) return malloc(n);
    size_t old = *(size_t *)((unsigned char *)p - 16);
    if (n <= old) return p;
    void *np = malloc(n);
    if (!np) return (void *)0;
    memcpy(np, p, old);
    return np;
}

void *memcpy(void *d, const void *s, size_t n) {
    unsigned char *dd = (unsigned char *)d;
    const unsigned char *ss = (const unsigned char *)s;
    for (size_t i = 0; i < n; i++) dd[i] = ss[i];
    return d;
}

void *memmove(void *d, const void *s, size_t n) {
    unsigned char *dd = (unsigned char *)d;
    const unsigned char *ss = (const unsigned char *)s;
    if (dd < ss) { for (size_t i = 0; i < n; i++) dd[i] = ss[i]; }
    else { for (size_t i = n; i > 0; i--) dd[i - 1] = ss[i - 1]; }
    return d;
}

void *memset(void *d, int c, size_t n) {
    unsigned char *dd = (unsigned char *)d;
    for (size_t i = 0; i < n; i++) dd[i] = (unsigned char)c;
    return d;
}

int memcmp(const void *a, const void *b, size_t n) {
    const unsigned char *x = (const unsigned char *)a;
    const unsigned char *y = (const unsigned char *)b;
    for (size_t i = 0; i < n; i++) { if (x[i] != y[i]) return (int)x[i] - (int)y[i]; }
    return 0;
}

size_t strlen(const char *s) { size_t n = 0; while (s[n]) n++; return n; }

int strcmp(const char *a, const char *b) {
    while (*a && *a == *b) { a++; b++; }
    return (int)(unsigned char)*a - (int)(unsigned char)*b;
}

int strncmp(const char *a, const char *b, size_t n) {
    for (size_t i = 0; i < n; i++) {
        unsigned char ca = (unsigned char)a[i], cb = (unsigned char)b[i];
        if (ca != cb) return (int)ca - (int)cb;
        if (ca == 0) break;
    }
    return 0;
}

char *strchr(const char *s, int c) {
    char ch = (char)c;
    for (;; s++) {
        if (*s == ch) return (char *)s;
        if (!*s) return (char *)0;
    }
}

char *strstr(const char *hay, const char *needle) {
    if (!*needle) return (char *)hay;
    for (; *hay; hay++) {
        const char *a = hay, *b = needle;
        while (*a && *b && *a == *b) { a++; b++; }
        if (!*b) return (char *)hay;
    }
    return (char *)0;
}

static void rx_byteswap(unsigned char *a, unsigned char *b, size_t n) {
    for (size_t i = 0; i < n; i++) { unsigned char t = a[i]; a[i] = b[i]; b[i] = t; }
}

/* Insertion sort — O(n^2) but correct and tiny; a faster sort is a follow-up. */
void qsort(void *base, size_t n, size_t sz, int (*cmp)(const void *, const void *)) {
    unsigned char *b = (unsigned char *)base;
    for (size_t i = 1; i < n; i++)
        for (size_t j = i; j > 0 && cmp(b + (j - 1) * sz, b + j * sz) > 0; j--)
            rx_byteswap(b + (j - 1) * sz, b + j * sz, sz);
}

/* ---- stdio/stdlib error-path stubs (fmt.c / string.c OOM paths) ---- */
void *stderr = (void *)0;                /* opaque dummy FILE* */
int errno = 0;
void exit(int code) { (void)code; __builtin_trap(); }
int fprintf(void *stream, const char *fmt, ...) { (void)stream; (void)fmt; return 0; }

/* round half away from zero (string.c float formatting) */
double round(double x) {
    return (x >= 0.0) ? (double)(long long)(x + 0.5) : -(double)(long long)(-x + 0.5);
}

/* strtoll — [ws][sign]digits in `base` (string.c passes base 10). Overflow is
 * not flagged (errno untouched); fine for in-range GUI inputs. */
long long strtoll(const char *s, char **endptr, int base) {
    const char *p = s;
    while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') p++;
    int neg = 0;
    if (*p == '+' || *p == '-') { neg = (*p == '-'); p++; }
    if (base == 0) base = 10;
    long long v = 0;
    for (;;) {
        int d;
        if (*p >= '0' && *p <= '9') d = *p - '0';
        else if (*p >= 'a' && *p <= 'z') d = *p - 'a' + 10;
        else if (*p >= 'A' && *p <= 'Z') d = *p - 'A' + 10;
        else break;
        if (d >= base) break;
        v = v * base + d;
        p++;
    }
    if (neg) v = -v;
    if (endptr) *endptr = (char *)p;
    return v;
}

/* strtoul — unsigned sibling of strtoll (string.c passes base 10). */
unsigned long strtoul(const char *s, char **endptr, int base) {
    const char *p = s;
    while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') p++;
    if (*p == '+') p++;
    if (base == 0) base = 10;
    unsigned long v = 0;
    for (;;) {
        int d;
        if (*p >= '0' && *p <= '9') d = *p - '0';
        else if (*p >= 'a' && *p <= 'z') d = *p - 'a' + 10;
        else if (*p >= 'A' && *p <= 'Z') d = *p - 'A' + 10;
        else break;
        if (d >= base) break;
        v = v * (unsigned long)base + (unsigned long)d;
        p++;
    }
    if (endptr) *endptr = (char *)p;
    return v;
}

/* ---- strtod: [ws][sign]digits[.digits][(e|E)[sign]digits] (string.c) ---- */
double strtod(const char *s, char **endptr) {
    const char *p = s;
    while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') p++;
    int neg = 0;
    if (*p == '+' || *p == '-') { neg = (*p == '-'); p++; }
    double val = 0.0;
    while (*p >= '0' && *p <= '9') { val = val * 10.0 + (double)(*p - '0'); p++; }
    if (*p == '.') {
        p++;
        double frac = 0.0, scale = 1.0;
        while (*p >= '0' && *p <= '9') { frac = frac * 10.0 + (double)(*p - '0'); scale *= 10.0; p++; }
        val += frac / scale;
    }
    if (*p == 'e' || *p == 'E') {
        p++;
        int eneg = 0;
        if (*p == '+' || *p == '-') { eneg = (*p == '-'); p++; }
        int ex = 0;
        while (*p >= '0' && *p <= '9') { ex = ex * 10 + (*p - '0'); p++; }
        double ep = 1.0;
        for (int i = 0; i < ex; i++) ep *= 10.0;
        if (eneg) val /= ep; else val *= ep;
    }
    if (neg) val = -val;
    if (endptr) *endptr = (char *)p;
    return val;
}

/* ---- minimal snprintf: %d/%i/%u/%x/%X (with l/ll), %f/%.*f/%.Nf, %g, %s, %c,
 * %% — covers string.c (PRId64, %g, %.*f) + fmt.c. Integer paths are exact;
 * float paths are reasonable (a fuller printf is a follow-up). ---- */
static int rx_utoa(unsigned long long v, unsigned base, int upper, char *out) {
    char tmp[24]; int n = 0;
    const char *dig = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    if (v == 0) tmp[n++] = '0';
    while (v) { tmp[n++] = dig[v % base]; v /= base; }
    for (int i = 0; i < n; i++) out[i] = tmp[n - 1 - i];
    return n;
}

static int rx_ftoa(double f, int prec, char *out) {
    int n = 0;
    if (f != f) { out[0] = 'n'; out[1] = 'a'; out[2] = 'n'; return 3; }
    if (f < 0) { out[n++] = '-'; f = -f; }
    double scale = 1.0;
    for (int i = 0; i < prec; i++) scale *= 10.0;
    unsigned long long ip = (unsigned long long)f;
    double frac = f - (double)ip;
    unsigned long long fp = (unsigned long long)(frac * scale + 0.5);
    if (prec > 0 && fp >= (unsigned long long)scale) { ip++; fp -= (unsigned long long)scale; }
    n += rx_utoa(ip, 10, 0, out + n);
    if (prec > 0) {
        out[n++] = '.';
        char fb[24]; int fn = rx_utoa(fp, 10, 0, fb);
        for (int i = 0; i < prec - fn; i++) out[n++] = '0';
        for (int i = 0; i < fn; i++) out[n++] = fb[i];
    }
    return n;
}

int snprintf(char *buf, size_t cap, const char *fmt, ...) {
    va_list ap; va_start(ap, fmt);
    size_t pos = 0;
#define RX_PUT(c) do { if (cap && pos + 1 < cap) buf[pos] = (c); pos++; } while (0)
    for (const char *p = fmt; *p; p++) {
        if (*p != '%') { RX_PUT(*p); continue; }
        p++;
        if (*p == '%') { RX_PUT('%'); continue; }
        while (*p == '-' || *p == '+' || *p == ' ' || *p == '0' || *p == '#') p++; /* flags (ignored) */
        while (*p >= '0' && *p <= '9') p++;                                        /* width (ignored) */
        int prec = -1;
        if (*p == '.') {
            p++;
            if (*p == '*') { prec = va_arg(ap, int); p++; }
            else { prec = 0; while (*p >= '0' && *p <= '9') { prec = prec * 10 + (*p - '0'); p++; } }
        }
        int lng = 0;
        while (*p == 'l') { lng++; p++; }
        while (*p == 'h' || *p == 'z') p++; /* other length mods (ignored) */
        char num[40]; int nn;
        switch (*p) {
            case 'd': case 'i': {
                long long v = (lng >= 2) ? va_arg(ap, long long)
                            : (lng == 1) ? (long long)va_arg(ap, long)
                                         : (long long)va_arg(ap, int);
                if (v < 0) { RX_PUT('-'); v = -v; }
                nn = rx_utoa((unsigned long long)v, 10, 0, num);
                for (int i = 0; i < nn; i++) RX_PUT(num[i]);
                break;
            }
            case 'u': {
                unsigned long long v = (lng >= 2) ? va_arg(ap, unsigned long long)
                                     : (lng == 1) ? (unsigned long long)va_arg(ap, unsigned long)
                                                  : (unsigned long long)va_arg(ap, unsigned);
                nn = rx_utoa(v, 10, 0, num); for (int i = 0; i < nn; i++) RX_PUT(num[i]); break;
            }
            case 'x': case 'X': {
                unsigned long long v = (lng >= 2) ? va_arg(ap, unsigned long long)
                                     : (lng == 1) ? (unsigned long long)va_arg(ap, unsigned long)
                                                  : (unsigned long long)va_arg(ap, unsigned);
                nn = rx_utoa(v, 16, *p == 'X', num); for (int i = 0; i < nn; i++) RX_PUT(num[i]); break;
            }
            case 'f': case 'F': {
                double v = va_arg(ap, double);
                nn = rx_ftoa(v, prec < 0 ? 6 : prec, num); for (int i = 0; i < nn; i++) RX_PUT(num[i]); break;
            }
            case 'g': case 'G': {
                double v = va_arg(ap, double);
                nn = rx_ftoa(v, 6, num);
                while (nn > 0 && num[nn - 1] == '0') nn--;     /* trim trailing zeros */
                if (nn > 0 && num[nn - 1] == '.') nn--;        /* and a dangling '.' */
                for (int i = 0; i < nn; i++) RX_PUT(num[i]); break;
            }
            case 's': { const char *s = va_arg(ap, const char *); if (!s) s = "(null)"; while (*s) RX_PUT(*s++); break; }
            case 'c': { int c = va_arg(ap, int); RX_PUT((char)c); break; }
            default: RX_PUT('%'); if (*p) RX_PUT(*p); break;
        }
    }
#undef RX_PUT
    if (cap) buf[pos < cap ? pos : cap - 1] = '\0';
    va_end(ap);
    return (int)pos;
}
"#;

/// Heap-core runtime `.c` files (by basename) compiled for the wasm target. A
/// curated subset of the per-package `runtime/*.c` — matches the curated wasm
/// stdlib bootstrap (tier 4.09). Grows as more heap surface is wired (string, fmt).
// The heap-core stdlib runtime compiled+linked for wasm. All the libc these
// need (malloc family, mem*/str*, qsort, snprintf, strtod, and fprintf/exit
// stubs) lives in the WASM_RT_C shim. fmt.c/string.c error paths trap via the
// exit stub. Grows as more of the stdlib is needed on wasm (tier 4.09).
pub const WASM_RUNTIME_CORE: &[&str] = &["alloc.c", "vec.c", "string.c", "fmt.c", "hash.c"];

/// Discover the C compiler used to build the wasm runtime: `RUXEN_WASM_CLANG`
/// override → `clang` on `PATH` (the LLVM-18 prefix should be on PATH where the
/// wasm backend already assumes LLVM 18).
fn find_wasm_clang() -> String {
    std::env::var("RUXEN_WASM_CLANG")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "clang".to_string())
}

/// Compile a single C source to a `wasm32-unknown-unknown` object with clang.
/// Freestanding (`-nostdlib`), `-fno-builtin` so the shim's `mem*`/`qsort` don't
/// self-recurse and so the stdlib runtime's `mem*` calls resolve to the shim.
fn compile_one_wasm_c(src: &Path, label: &str) -> Result<PathBuf, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or(label);
    let out = std::env::temp_dir().join(format!(
        "ruxen_wasm_{}_{}_{}.o",
        stem,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    ));
    let clang = find_wasm_clang();
    let mut cmd = Command::new(&clang);
    cmd.arg("--target=wasm32-unknown-unknown")
        .arg("-nostdlib")
        .arg("-fno-builtin")
        .arg("-O2")
        .arg("-c")
        .arg(src)
        .arg("-o")
        .arg(&out);
    let status = cmd.status().map_err(|e| {
        format!(
            "Failed to invoke '{}' to compile {} for wasm: {}. \
             Install clang (LLVM 18) or set RUXEN_WASM_CLANG.",
            clang,
            src.display(),
            e
        )
    })?;
    if !status.success() {
        return Err(format!(
            "wasm C compile failed for {} ({})",
            src.display(),
            label
        ));
    }
    Ok(out)
}

/// Compile one heap-core runtime `.c` (e.g. `vec.c`) for wasm. See [`compile_one_wasm_c`].
pub fn compile_runtime_for_wasm(src: &Path) -> Result<PathBuf, String> {
    compile_one_wasm_c(src, "runtime")
}

/// Materialize [`WASM_RT_C`] to a temp file and compile it for wasm. Returns the
/// object path (caller links it, then `emit_wasm_module` cleans it up).
pub fn compile_wasm_shim() -> Result<PathBuf, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let c_path = std::env::temp_dir().join(format!(
        "ruxen_wasm_shim_{}_{}.c",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::write(&c_path, WASM_RT_C)
        .map_err(|e| format!("Failed to write wasm runtime shim: {}", e))?;
    let obj = compile_one_wasm_c(&c_path, "wasm_rt");
    let _ = std::fs::remove_file(&c_path);
    obj
}

/// Two-stage Docker link: emit the target object locally, then compile the
/// stdlib runtime `.c` *and* link inside a target-native container.
///
/// This is the honest fallback for Linux targets from a macOS host with no
/// cross toolchain installed (`zig`/`aarch64-linux-gnu-gcc` absent). The
/// container's native `cc` + libc compile the runtime for the target and link
/// the final binary — no host cross-cc required.
///
/// Mounting strategy: the stdlib root (`library/std`) is mounted read-only at
/// `/std` **preserving directory structure**, so each runtime `.c`'s relative
/// `#include "../../core/runtime/runtime.h"` resolves exactly as it does on
/// the host. A writable scratch dir is mounted at `/work` for the staged
/// object, per-source `.o`s, and the final binary. PCRE2 vendor sources get
/// the same include/width flags as the host (`apply_pcre2_flags` analogue,
/// inlined into the in-container shell). User-supplied runtime sources (which
/// live outside the stdlib tree) are staged flat into `/work` — they are
/// standalone TUs by the project convention.
///
/// Requires Docker. The error when Docker is missing is actionable (points at
/// the manifest linker override) — never a silent failure.
fn emit_executable_in_container(
    object_bytes: &[u8],
    runtime_sources: &[PathBuf],
    output_path: &str,
    target: &ResolvedTarget,
    extra_link_flags: &[String],
) -> Result<(), String> {
    use crate::codegen::target::docker_platform;

    let platform = docker_platform(target);
    let stdlib_root = super::find_stdlib_root()?;
    let stdlib_root = stdlib_root
        .canonicalize()
        .map_err(|e| format!("Failed to canonicalize stdlib root: {}", e))?;

    let scratch = std::env::temp_dir().join(format!(
        "ruxen_xlink_{}_{}",
        std::process::id(),
        target.canonical().replace('-', "_")
    ));
    std::fs::create_dir_all(&scratch)
        .map_err(|e| format!("Failed to create cross-link scratch dir: {}", e))?;
    let guard = ScratchGuard(scratch.clone());

    let obj_name = "ruxen_main.o";
    std::fs::write(scratch.join(obj_name), object_bytes)
        .map_err(|e| format!("Failed to stage object: {}", e))?;

    // Partition runtime sources: stdlib (under stdlib_root → compiled in place
    // at /std/<rel>) vs. user (outside → staged flat into /work).
    let mut compile_lines: Vec<String> = Vec::new();
    let mut object_names: Vec<String> = Vec::new();
    let mut user_staged = 0usize;
    for src in runtime_sources {
        let canon = src
            .canonicalize()
            .map_err(|e| format!("runtime source {} missing: {}", src.display(), e))?;
        let is_pcre2 = canon.components().any(|c| c.as_os_str() == "pcre2");
        let pcre2_flags = if is_pcre2 {
            // Vendor dir is the .c's own parent inside the container.
            "-I\"$(dirname \"$SRC\")\" -DHAVE_CONFIG_H=1 -DPCRE2_CODE_UNIT_WIDTH=8 \
             -Wno-sign-compare -Wno-unused-parameter -Wno-implicit-fallthrough"
        } else {
            ""
        };

        let (container_src, obj) = if let Ok(rel) = canon.strip_prefix(&stdlib_root) {
            // Stdlib source: compiled in place under the read-only /std mount.
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            let obj = format!("/work/std_{}.o", user_obj_safe(&rel_str));
            (format!("/std/{}", rel_str), obj)
        } else {
            // User source: stage flat into scratch.
            let fname = canon
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| format!("bad runtime source path {}", canon.display()))?;
            std::fs::copy(&canon, scratch.join(fname))
                .map_err(|e| format!("Failed to stage user runtime {}: {}", canon.display(), e))?;
            user_staged += 1;
            let obj = format!("/work/user_{}.o", user_obj_safe(fname));
            (format!("/work/{}", fname), obj)
        };

        // `SRC=...; cc -c "$SRC" ...` so the PCRE2 `-I$(dirname $SRC)` resolves.
        compile_lines.push(format!(
            "SRC={}; cc -O2 {} -c \"$SRC\" -o {}",
            shell_quote(&container_src),
            pcre2_flags,
            shell_quote(&obj)
        ));
        object_names.push(obj);
    }
    let _ = user_staged;

    let out_name = Path::new(output_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("a.out");

    // Build the in-container script: compile each runtime source, then link
    // the staged main object + every runtime object into the final binary.
    let mut script = String::from("set -e\n");
    for line in &compile_lines {
        script.push_str(line);
        script.push('\n');
    }
    script.push_str("cc -O2 /work/");
    script.push_str(obj_name);
    for obj in &object_names {
        script.push(' ');
        script.push_str(&shell_quote(obj));
    }
    script.push_str(" -o /work/");
    script.push_str(out_name);
    for flag in extra_link_flags {
        // Drop macOS-only framework flags that don't exist on Linux.
        if flag == "-framework" || flag == "Security" {
            continue;
        }
        script.push(' ');
        script.push_str(&shell_quote(flag));
    }
    script.push('\n');

    let work_mount = format!("{}:/work", scratch.display());
    let std_mount = format!("{}:/std:ro", stdlib_root.display());
    let mut cmd = Command::new("docker");
    cmd.arg("run")
        .arg("--rm")
        .arg("--platform")
        .arg(platform)
        .arg("-v")
        .arg(&work_mount)
        .arg("-v")
        .arg(&std_mount)
        .arg("-w")
        .arg("/work")
        // A small image with a C toolchain. gcc:13 ships cc + glibc + libm.
        .arg("gcc:13")
        .arg("bash")
        .arg("-c")
        .arg(&script);

    let output = cmd.output().map_err(|e| {
        format!(
            "Failed to invoke docker for container cross-link of '{}': {}. \
             Install Docker or set [target.{}].linker in Ruxen.toml.",
            target.canonical(),
            e,
            target.canonical()
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "Container cross-link failed for '{}' (target {}):\n{}",
            output_path,
            target.canonical(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Copy the produced binary out of the scratch mount to the final path.
    std::fs::copy(scratch.join(out_name), output_path)
        .map_err(|e| format!("Cross-link produced no binary for '{}': {}", output_path, e))?;
    // Preserve the executable bit.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(output_path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(output_path, perms);
        }
    }
    drop(guard);
    Ok(())
}

/// RAII cleanup for the container-link scratch directory.
struct ScratchGuard(PathBuf);
impl Drop for ScratchGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Turn an arbitrary path-ish string into a flat, filesystem-safe object stem
/// (so two runtime sources with the same basename in different package dirs
/// don't collide in `/work`).
fn user_obj_safe(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Single-quote a string for a POSIX `bash -c` script (closes/reopens around
/// embedded single quotes). Inputs here are paths/flags we control, but
/// quoting keeps spaces and shell metacharacters inert.
fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::linker_args;

    // B3 of docs/specs/system/zero_rust_stdlib_classes.spec.md moved
    // `-lc / -lm / -lpthread` out of `linker_args` into per-package
    // Ruxen.toml `[system_libs]` tables. The aggregation now happens
    // upstream in `codegen::compile_with_options` and arrives here
    // through the `extra_link_flags` argument — so `linker_args(_,
    // &["-lpthread", "-lc", "-lm"])` is now the right contract to
    // assert. See `compiler/ruxen_core/tests/linker_system_libs.rs`
    // for the package-walk pin.
    #[test]
    fn linker_args_propagate_pthread_through_extra_flags() {
        let args = linker_args(false, &[String::from("-lpthread")]);
        assert!(
            args.iter().any(|arg| arg == "-lpthread"),
            "extra_link_flags must reach the final arg list; got {args:?}"
        );
    }

    #[test]
    fn linker_args_preserve_sanitizers_and_extra_flags() {
        let args = linker_args(
            true,
            &[
                String::from("-lpthread"),
                String::from("-lssl"),
                String::from("-lcrypto"),
            ],
        );
        assert!(args.iter().any(|arg| arg == "-lpthread"));
        assert!(args.iter().any(|arg| arg == "-fsanitize=address,undefined"));
        assert!(args.iter().any(|arg| arg == "-lssl"));
        assert!(args.iter().any(|arg| arg == "-lcrypto"));
    }

    /// Phase 2 #06.5 T8: macOS link must include `-framework Security`
    /// so SecRandomCopyBytes (used by std::rand) resolves at link
    /// time. The flag is `cfg!(target_os = "macos")`-gated; this test
    /// only enforces the contract on macOS hosts.
    #[cfg(target_os = "macos")]
    #[test]
    fn linker_args_include_security_framework_on_macos() {
        let args = linker_args(false, &[]);
        let security_pair = args
            .windows(2)
            .any(|w| w[0] == "-framework" && w[1] == "Security");
        assert!(
            security_pair,
            "macOS linker_args must emit `-framework Security` as a pair; got {args:?}"
        );
    }
}
