#!/usr/bin/env python3
"""Memory run of the loan matrix: compile every program with the release
`ruxen`, skip the ones the checker rejects, and run the rest under the
platform memory checker. Exits 1 when an accepted program crashes, exits
non-zero, gets a memory-checker report, prints other output than its
`.expect`, or the compiler itself crashes. Leaks are reported, never fatal.

usage: memrun.py [--checker gmalloc|asan] [--leaks] [--jobs N]
                 [--results FILE] [PROGRAM.rx | DIR]...

Checkers: `gmalloc` (macOS default) runs each binary with Guard Malloc,
which faults on any read or write of freed memory. `asan` (Linux default,
also works on macOS) links an AddressSanitizer+UBSan build of the C
runtime: the script compiles the stdlib runtime sources once into
`<OUT>/asan/libruxenrt-asan.a`, points `RUXEN_RUNTIME_AR` at it and puts a
`cc` wrapper that adds the sanitizer flags first on PATH, so the
compiler's own link pulls in the sanitizer runtime. On Linux LeakSanitizer
reports leaks; on macOS `--leaks` runs `leaks --atExit` (slow).

Environment: RX (compiler, default <repo>/target/release/ruxen), OUT
(scratch dir, default <repo>/tmp/test-cache/loan-matrix-run). Each binary
is deleted after its run."""
import argparse
import concurrent.futures
import os
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "../../../../.."))
SAN_FLAGS = ["-fsanitize=address,undefined", "-fno-omit-frame-pointer", "-g"]
# Sanitizer, UBSan, system malloc and Guard Malloc error lines (Guard
# Malloc's start-up banner is not one).
MEMORY_REPORT = re.compile(r"ERROR: AddressSanitizer|runtime error:|malloc: \*\*\*|GuardMalloc\[[^\]]*\]: \*\*\*")


def programs(paths):
    found = []
    for p in paths or [HERE]:
        if os.path.isdir(p):
            found += [os.path.join(p, f) for f in os.listdir(p) if f.endswith(".rx")]
        else:
            found.append(p)
    return sorted({os.path.abspath(p) for p in found})


def runtime_sources():
    """The stdlib C runtime, as `codegen::find_runtime_sources` collects it:
    library/std/<pkg>/runtime/*.c plus one vendored subdirectory level,
    without PCRE2's two included-only files."""
    std = os.path.join(REPO, "library/std")
    sources = []
    for pkg in sorted(os.listdir(std)):
        rt = os.path.join(std, pkg, "runtime")
        if not os.path.isdir(rt):
            continue
        for entry in sorted(os.listdir(rt)):
            path = os.path.join(rt, entry)
            if entry.endswith(".c"):
                sources.append(path)
            elif os.path.isdir(path):
                sources += [os.path.join(path, f) for f in sorted(os.listdir(path))
                            if f.endswith(".c") and f not in ("pcre2_printint.c", "pcre2_ucptables.c")]
    return sources


def asan_env(out, jobs):
    """Build the sanitized runtime archive and the `cc` wrapper once; return
    the environment a sanitized compile needs."""
    real_cc = shutil.which("cc")
    work = os.path.join(out, "asan")
    os.makedirs(work, exist_ok=True)
    archive = os.path.join(work, "libruxenrt-asan.a")
    if not os.path.exists(archive):
        def compile_one(i_src):
            i, src = i_src
            obj = os.path.join(work, f"{i:03d}_{os.path.basename(src)[:-2]}.o")
            cmd = [real_cc, "-c", src, "-o", obj, "-O1"] + SAN_FLAGS
            if sys.platform == "darwin":
                cmd.append("-mmacosx-version-min=11.0")
            if os.path.basename(os.path.dirname(src)) == "pcre2":
                cmd += ["-I", os.path.dirname(src), "-DHAVE_CONFIG_H=1", "-DPCRE2_CODE_UNIT_WIDTH=8",
                        "-Wno-sign-compare", "-Wno-unused-parameter", "-Wno-implicit-fallthrough"]
            subprocess.run(cmd, check=True)
            return obj
        with concurrent.futures.ThreadPoolExecutor(jobs) as pool:
            objects = list(pool.map(compile_one, enumerate(runtime_sources())))
        subprocess.run(["ar", "rcs", archive] + objects, check=True)
        for obj in objects:
            os.remove(obj)
    wrapper = os.path.join(work, "cc")
    with open(wrapper, "w") as f:
        f.write(f'#!/bin/sh\nexec "{real_cc}" "$@" {" ".join(SAN_FLAGS)}\n')
    os.chmod(wrapper, 0o755)
    env = dict(os.environ)
    env["PATH"] = work + os.pathsep + env["PATH"]
    env["RUXEN_RUNTIME_AR"] = archive
    # LeakSanitizer exists on Linux only; it reports but keeps exit 0.
    env["ASAN_OPTIONS"] = "detect_leaks=%d" % (sys.platform != "darwin")
    env["LSAN_OPTIONS"] = "exitcode=0"
    env["UBSAN_OPTIONS"] = "print_stacktrace=1"
    return env


def run_one(path, rx, out, checker, compile_env, leaks):
    name = os.path.basename(path)[:-3]
    binary = os.path.join(out, name + ".bin")
    try:
        c = subprocess.run([rx, "compile", path, "-o", binary, "--force"], cwd=out, env=compile_env,
                           capture_output=True, text=True, timeout=180)
    except subprocess.TimeoutExpired:
        return name, "COMPILER-CRASH", "compile timed out"
    text = c.stdout + c.stderr
    if c.returncode != 0 or not os.path.exists(binary):
        if c.returncode < 0 or "panicked" in text:
            return name, "COMPILER-CRASH", text.strip().splitlines()[-1][:160] if text.strip() else ""
        codes = ",".join(sorted(set(re.findall(r"E\d{4}", text))))
        return name, "REJECT", codes
    env = dict(compile_env) if checker == "asan" else dict(os.environ)
    if checker == "gmalloc":
        env["DYLD_INSERT_LIBRARIES"] = "/usr/lib/libgmalloc.dylib"
        env["MALLOC_STRICT_SIZE"] = "1"
    try:
        r = subprocess.run([binary], cwd=out, env=env, capture_output=True, text=True, timeout=60)
        rc, stdout, stderr = r.returncode, r.stdout, r.stderr
    except subprocess.TimeoutExpired:
        rc, stdout, stderr = "timeout", "", ""
    leak = ""
    m = re.search(r"SUMMARY: AddressSanitizer: (\d+) byte\(s\) leaked in (\d+) allocation", stderr)
    if m:
        leak = f"{m.group(2)} leaked allocations"
    if leaks and sys.platform == "darwin" and rc == 0:
        lk = subprocess.run(["leaks", "--atExit", "--", binary], cwd=out, capture_output=True, text=True,
                            timeout=180)
        m = re.search(r"(\d+) leaks? for \d+ total", lk.stdout)
        if m and m.group(1) != "0":
            leak = m.group(0)
    os.remove(binary)
    expect_path = path[:-3] + ".expect"
    report = MEMORY_REPORT.search(stderr)
    if rc == "timeout":
        status = "TIMEOUT"
    elif report:
        status = "MEMORY-ERROR"
    elif rc < 0 or rc >= 128:
        status = "CRASH"
    elif rc != 0:
        status = "EXIT"
    elif os.path.exists(expect_path) and stdout != open(expect_path).read():
        status = "WRONG-OUTPUT"
    else:
        status = "CLEAN"
    detail = f"exit={rc}"
    if report:
        detail += " " + stderr[report.start():].splitlines()[0][:120]
    return name, status, (detail + (f" leaks: {leak}" if leak else "")).strip()


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("paths", nargs="*")
    ap.add_argument("--checker", choices=["gmalloc", "asan"],
                    default="gmalloc" if sys.platform == "darwin" else "asan")
    ap.add_argument("--leaks", action="store_true", help="macOS: also run `leaks --atExit`")
    ap.add_argument("--jobs", type=int, default=os.cpu_count())
    ap.add_argument("--results", help="write one `name|status|detail` line per program")
    args = ap.parse_args()
    rx = os.environ.get("RX", os.path.join(REPO, "target/release/ruxen"))
    out = os.environ.get("OUT", os.path.join(REPO, "tmp/test-cache/loan-matrix-run"))
    os.makedirs(out, exist_ok=True)
    env = asan_env(out, args.jobs) if args.checker == "asan" else dict(os.environ)
    files = programs(args.paths)
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        results = list(pool.map(lambda p: run_one(p, rx, out, args.checker, env, args.leaks), files))
    if args.results:
        with open(args.results, "w") as f:
            f.writelines(f"{n}|{s}|{d}\n" for n, s, d in results)
    by = {}
    for name, status, _ in results:
        by.setdefault(status, []).append(name)
    fatal = [r for r in results if r[1] not in ("REJECT", "CLEAN")]
    accepted = [r for r in results if r[1] != "REJECT"]
    print(f"{len(files)} programs ({args.checker}, {rx}): " +
          ", ".join(f"{len(v)} {k.lower()}" for k, v in sorted(by.items())))
    for name, status, detail in results:
        if status == "REJECT" and not name.endswith("__conflict"):
            print(f"note: dead row rejected {name} {detail}")
        elif status == "CLEAN" and name.endswith("__conflict"):
            print(f"note: conflict accepted, ran clean {name} {detail}")
        elif "leak" in detail and status == "CLEAN":
            print(f"leak: {name} {detail}")
    for name, status, detail in fatal:
        print(f"FAIL {status} {name} {detail}")
    print(f"{len(accepted)} accepted, {len(fatal)} failed")
    return 1 if fatal else 0


if __name__ == "__main__":
    sys.exit(main())
