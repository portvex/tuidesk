//! Release tooling: `cargo xtask dist [KIND...] [--out DIR]` builds tuidesk for every platform
//! and wraps it in each platform's own package format. Run `cargo xtask help` for the kinds.
//!
//! Each kind calls the tool that format is normally built with (cargo-zigbuild, cargo-deb,
//! cargo-generate-rpm, lipo, makensis, git); this program only decides the order, the targets
//! and the file names, and writes SHA256SUMS for everything it made.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256, Sha512};

const HELP: &str = "\
cargo xtask dist [KIND...] [--out DIR]

Builds release artifacts into DIR (default: dist/). KINDs, default `all`:
  linux     static musl executables (x86_64, aarch64) as .tar.gz      needs cargo-zigbuild, zig
  deb       Debian/Ubuntu packages of the static executables         needs cargo-deb
  rpm       Fedora/RHEL/openSUSE packages of the static executables  needs cargo-generate-rpm
  macos     universal (arm64 + x86_64) executable as .tar.gz         needs a macOS host
  windows   portable .zip and NSIS installer (x86_64, arm64)         needs cargo-zigbuild, makensis
  source    source tarball (what Arch, Alpine and Homebrew build from)
  recipes   PKGBUILD, APKBUILD and Homebrew formula for this version, with the source checksum
  all       every kind this host can build (macos only on macOS, NSIS only with makensis)

The Nix flake (flake.nix) builds from the repository itself and needs no artifact.
";

const LINUX: [&str; 2] = ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"];
const MACOS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];
/// (target, architecture name used in file names and by NSIS).
const WINDOWS: [(&str, &str); 2] = [("x86_64-pc-windows-gnu", "x86_64"), ("aarch64-pc-windows-gnullvm", "arm64")];
const MAN_PAGES: [&str; 3] = ["man/tuidesk.1", "man/tuidesk-config.5", "man/tuidesk-lua.7"];

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("dist") => {}
        None | Some("help" | "-h" | "--help") => {
            print!("{HELP}");
            return Ok(());
        }
        Some(other) => bail!("unknown task '{other}' (see: cargo xtask help)"),
    }
    let mut kinds = Vec::new();
    let mut out = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(args.next().context("--out needs a directory")?)),
            "linux" | "deb" | "rpm" | "macos" | "windows" | "source" | "recipes" | "all" => kinds.push(a),
            other => bail!("unknown kind '{other}' (see: cargo xtask help)"),
        }
    }
    let explicit = !kinds.is_empty() && !kinds.iter().any(|k| k == "all");
    if !explicit {
        kinds = ["linux", "deb", "rpm", "macos", "windows", "source", "recipes"].map(String::from).to_vec();
    }
    let d = Dist::new(out, explicit)?;
    fs::create_dir_all(&d.out)?;
    for kind in &kinds {
        println!("== {kind}");
        match kind.as_str() {
            "linux" => d.linux()?,
            "deb" => d.deb()?,
            "rpm" => d.rpm()?,
            "macos" => d.macos()?,
            "windows" => d.windows()?,
            "source" => d.source()?,
            "recipes" => d.recipes()?,
            _ => unreachable!(),
        }
    }
    d.checksums()
}

struct Dist {
    root: PathBuf,
    out: PathBuf,
    version: String,
    repository: String,
    /// Kinds were named on the command line: a missing tool is an error, not a skip.
    explicit: bool,
}

impl Dist {
    fn new(out: Option<PathBuf>, explicit: bool) -> Result<Self> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().context("xtask has no parent")?.to_path_buf();
        let manifest: toml::Table = toml::from_str(&fs::read_to_string(root.join("Cargo.toml"))?)?;
        let package = manifest["package"].as_table().context("no [package]")?;
        let field = |k: &str| package.get(k).and_then(|v| v.as_str()).map(str::to_string);
        // A relative --out is taken from where the command runs, like any other path argument.
        let out = match out {
            Some(o) => std::env::current_dir()?.join(o),
            None => root.join("dist"),
        };
        Ok(Self {
            version: field("version").context("no package.version")?,
            repository: field("repository").context("no package.repository")?,
            root,
            out,
            explicit,
        })
    }

    fn name(&self, rest: &str) -> String {
        format!("tuidesk-{}-{rest}", self.version)
    }

    /// Run `program args...` in the repository root.
    fn run(&self, program: &str, args: &[&str]) -> Result<()> {
        println!("$ {program} {}", args.join(" "));
        let status = Command::new(program)
            .args(args)
            .current_dir(&self.root)
            .status()
            .with_context(|| format!("running {program} (is it installed?)"))?;
        if !status.success() {
            bail!("{program} failed ({status})");
        }
        Ok(())
    }

    /// Whether `program` can be run; when it cannot, skip the step (or fail if it was asked for).
    fn have(&self, program: &str, args: &[&str], what: &str) -> Result<bool> {
        let ok = Command::new(program).args(args).output().is_ok_and(|o| o.status.success());
        match (ok, self.explicit) {
            (true, _) => Ok(true),
            (false, true) => bail!("{what} needs {program}, which is not installed"),
            (false, false) => {
                println!("skipping {what}: {program} is not installed");
                Ok(false)
            }
        }
    }

    fn binary(&self, target: &str, exe: &str) -> PathBuf {
        self.root.join("target").join(target).join("release").join(exe)
    }

    fn zigbuild(&self, target: &str) -> Result<()> {
        self.run("cargo", &["zigbuild", "--release", "--locked", "--target", target])
    }

    fn linux(&self) -> Result<()> {
        for t in LINUX {
            self.zigbuild(t)?;
            self.tarball(&self.name(t), &self.binary(t, "tuidesk"))?;
        }
        Ok(())
    }

    fn deb(&self) -> Result<()> {
        if !self.have("cargo", &["deb", "--version"], "deb")? {
            return Ok(());
        }
        let out = self.out.to_string_lossy();
        for t in LINUX {
            // Up to date after `linux`, so this only builds when that kind did not run.
            self.zigbuild(t)?;
            // Already stripped by the release profile; the host's strip may not read Linux ELF.
            self.run("cargo", &["deb", "--no-build", "--no-strip", "--target", t, "--output", &out])?;
        }
        Ok(())
    }

    fn rpm(&self) -> Result<()> {
        if !self.have("cargo", &["generate-rpm", "--version"], "rpm")? {
            return Ok(());
        }
        let out = self.out.to_string_lossy();
        for t in LINUX {
            // Up to date after `linux`, so this only builds when that kind did not run.
            self.zigbuild(t)?;
            // A static executable needs no shared libraries, and ldd is not on every host.
            self.run("cargo", &["generate-rpm", "--target", t, "--auto-req", "disabled", "--output", &out])?;
        }
        Ok(())
    }

    fn macos(&self) -> Result<()> {
        if !cfg!(target_os = "macos") {
            if self.explicit {
                bail!("macos needs a macOS host (Apple's SDK and lipo)");
            }
            println!("skipping macos: not a macOS host");
            return Ok(());
        }
        for t in MACOS {
            self.run("cargo", &["build", "--release", "--locked", "--target", t])?;
        }
        let universal = self.root.join("target/universal-apple-darwin/release");
        fs::create_dir_all(&universal)?;
        let exe = universal.join("tuidesk");
        let parts: Vec<String> = MACOS.iter().map(|t| self.binary(t, "tuidesk").display().to_string()).collect();
        let exe_s = exe.display().to_string();
        self.run("lipo", &["-create", "-output", &exe_s, &parts[0], &parts[1]])?;
        self.tarball(&self.name("macos-universal"), &exe)
    }

    fn windows(&self) -> Result<()> {
        for (t, arch) in WINDOWS {
            self.zigbuild(t)?;
            self.portable_zip(&self.name(&format!("windows-{arch}-portable")), &self.binary(t, "tuidesk.exe"))?;
        }
        if !self.have("makensis", &["-VERSION"], "the Windows installer")? {
            return Ok(());
        }
        for (t, arch) in WINDOWS {
            let exe = self.binary(t, "tuidesk.exe");
            let outfile = self.out.join(self.name(&format!("windows-{arch}-setup.exe")));
            self.run(
                "makensis",
                &[
                    "-V2",
                    &format!("-DVERSION={}", self.version),
                    &format!("-DARCH={arch}"),
                    &format!("-DEXE={}", exe.display()),
                    &format!("-DROOT={}", self.root.display()),
                    &format!("-DOUTFILE={}", outfile.display()),
                    "packaging/windows/tuidesk.nsi",
                ],
            )?;
        }
        Ok(())
    }

    /// The source as committed. With uncommitted changes, the working tree as it would be
    /// committed (tracked and new files, minus .gitignore), so a local build can be tested.
    fn source(&self) -> Result<()> {
        let dirty = !git(&self.root, &["status", "--porcelain"], None)?.trim().is_empty();
        let tree = if dirty {
            println!("note: the working tree has uncommitted changes; the source tarball includes them");
            let index = self.root.join("target/xtask-index");
            let _ = fs::remove_file(&index);
            git(&self.root, &["add", "--all"], Some(&index))?;
            git(&self.root, &["write-tree"], Some(&index))?.trim().to_string()
        } else {
            "HEAD".to_string()
        };
        let file = self.source_tarball();
        let prefix = format!("--prefix=tuidesk-{}/", self.version);
        let output = format!("--output={}", file.display());
        git(&self.root, &["archive", "--format=tar.gz", &prefix, &output, &tree], None)?;
        println!("wrote {}", file.display());
        Ok(())
    }

    fn source_tarball(&self) -> PathBuf {
        self.out.join(format!("tuidesk-{}.tar.gz", self.version))
    }

    /// Package recipes for this version from `packaging/*/*.in`: @VERSION@, @URL@ (the source
    /// tarball on the release page) and its checksums @SHA256@ and @SHA512@ (Alpine's).
    fn recipes(&self) -> Result<()> {
        let tarball = self.source_tarball();
        if !tarball.exists() {
            self.source()?;
        }
        let bytes = fs::read(&tarball)?;
        let sha256 = hex(&Sha256::digest(&bytes));
        let sha512 = hex(&Sha512::digest(&bytes));
        let url = format!("{}/releases/download/v{v}/tuidesk-{v}.tar.gz", self.repository, v = self.version);
        for (template, name) in [
            ("packaging/arch/PKGBUILD.in", "arch/PKGBUILD"),
            ("packaging/alpine/APKBUILD.in", "alpine/APKBUILD"),
            ("packaging/homebrew/tuidesk.rb.in", "homebrew/Formula/tuidesk.rb"),
        ] {
            let text = fs::read_to_string(self.root.join(template)).with_context(|| format!("reading {template}"))?;
            let text = text
                .replace("@VERSION@", &self.version)
                .replace("@URL@", &url)
                .replace("@SHA256@", &sha256)
                .replace("@SHA512@", &sha512);
            let dest = self.out.join("packaging").join(name);
            fs::create_dir_all(dest.parent().unwrap())?;
            fs::write(&dest, text)?;
            println!("wrote {}", dest.display());
        }
        Ok(())
    }

    /// `<name>.tar.gz` with `<name>/`: the executable, its manual pages, LICENSE and README.
    fn tarball(&self, name: &str, exe: &Path) -> Result<()> {
        let file = self.out.join(format!("{name}.tar.gz"));
        let gz = flate2::write::GzEncoder::new(fs::File::create(&file)?, flate2::Compression::best());
        let mut tar = tar::Builder::new(gz);
        tar.mode(tar::HeaderMode::Deterministic);
        tar.append_path_with_name(exe, format!("{name}/tuidesk"))?;
        for f in MAN_PAGES.iter().chain(&["LICENSE", "README.md"]) {
            tar.append_path_with_name(self.root.join(f), format!("{name}/{f}"))?;
        }
        tar.into_inner()?.finish()?;
        println!("wrote {}", file.display());
        Ok(())
    }

    /// `<name>.zip`: tuidesk.exe, LICENSE.txt and README.md; it runs from wherever it is unpacked.
    fn portable_zip(&self, name: &str, exe: &Path) -> Result<()> {
        let file = self.out.join(format!("{name}.zip"));
        let mut zip = zip::ZipWriter::new(fs::File::create(&file)?);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (src, dest) in [
            (exe.to_path_buf(), "tuidesk.exe"),
            (self.root.join("LICENSE"), "LICENSE.txt"),
            (self.root.join("README.md"), "README.md"),
        ] {
            zip.start_file(format!("{name}/{dest}"), opts)?;
            zip.write_all(&fs::read(&src).with_context(|| format!("reading {}", src.display()))?)?;
        }
        zip.finish()?;
        println!("wrote {}", file.display());
        Ok(())
    }

    /// SHA256SUMS over the artifacts in the output directory, in `sha256sum -c` format.
    fn checksums(&self) -> Result<()> {
        let mut files: Vec<PathBuf> = fs::read_dir(&self.out)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && p.file_name().is_some_and(|n| n != "SHA256SUMS"))
            .collect();
        files.sort();
        let mut sums = String::new();
        for f in &files {
            sums += &format!("{}  {}\n", sha256(f)?, f.file_name().unwrap().to_string_lossy());
        }
        fs::write(self.out.join("SHA256SUMS"), sums)?;
        println!("wrote {} ({} files)", self.out.join("SHA256SUMS").display(), files.len());
        Ok(())
    }
}

fn git(root: &Path, args: &[&str], index: Option<&Path>) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(root);
    if let Some(i) = index {
        cmd.env("GIT_INDEX_FILE", i);
    }
    let out = cmd.output().context("running git")?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8(out.stdout)?)
}

fn sha256(path: &Path) -> Result<String> {
    Ok(hex(&Sha256::digest(fs::read(path).with_context(|| format!("reading {}", path.display()))?)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
