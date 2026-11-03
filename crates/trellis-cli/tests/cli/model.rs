use super::*;

// ---------------------------------------------------------------------------
// model — the weights catalogue, which needs no network to be wrong
// ---------------------------------------------------------------------------

/// Runs a command with the user's data directory pointed at `data`.
///
/// `models_dir` reads `LOCALAPPDATA` on Windows and `XDG_DATA_HOME` (then
/// `HOME`) elsewhere. Both are set, so the test says the same thing on
/// either platform and never touches the real one — writing a fake weights
/// file into someone's actual model directory is not a thing a test does.
fn run_with_data_dir(root: &Path, data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trellis"))
        .args(args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        // Hermetic: the user-level config and context files (ADR 0025 §7)
        // of whoever runs the suite must not change what it sees.
        .env(
            "TRELLIS_CONFIG_DIR",
            std::env::temp_dir().join("trellis-tests-no-user-config"),
        )
        .env("LOCALAPPDATA", data)
        .env("XDG_DATA_HOME", data)
        .env("HOME", data)
        .output()
        .expect("the binary must be built before its integration tests run")
}

/// Where `models_dir` will look, given that data directory.
fn models_dir_in(data: &Path) -> PathBuf {
    data.join("trellis").join("models")
}

#[test]
fn the_model_list_reports_each_weight_as_absent_partial_or_complete() {
    // Three states and a different action behind each: fetch it, resume it,
    // use it. Collapsing any two — which every mutant of the guard does —
    // makes the list say the wrong thing about work the user has already
    // paid for. On a slow link the difference between resuming and
    // restarting is hours, which is why a partial file is named rather than
    // hidden.
    //
    // No network is involved in any of this: the state is `fs::metadata`
    // against a size the catalogue already knows.
    let d = repo("model-list");
    let data = scratch("model-list-data");
    let dir = models_dir_in(&data);
    std::fs::create_dir_all(&dir).unwrap();

    // Nothing fetched yet.
    let s = stdout(&run_with_data_dir(&d, &data, &["model"]));
    assert!(
        s.contains("not fetched"),
        "an empty directory means nothing is fetched:\n{s}"
    );
    assert!(
        !s.contains("downloaded") && !s.contains("partial"),
        "and nothing is downloaded or partial:\n{s}"
    );

    // The smallest entry, part-way down.
    let entry = trellis_model::CATALOG
        .iter()
        .min_by_key(|e| e.bytes)
        .expect("the catalogue is not empty");
    let path = dir.join(entry.file);
    std::fs::write(&path, vec![0u8; 1024]).unwrap();
    let s = stdout(&run_with_data_dir(&d, &data, &["model"]));
    assert!(
        s.contains("partial"),
        "a file shorter than the published size is a resumable download, \
         not a usable weight:\n{s}"
    );

    // The same entry, complete. Written sparse: the file is half a gigabyte
    // and none of it is read, so `set_len` costs nothing and the point is
    // the length `metadata` reports.
    let f = std::fs::File::options().write(true).open(&path).unwrap();
    f.set_len(entry.bytes).unwrap();
    drop(f);
    let s = stdout(&run_with_data_dir(&d, &data, &["model"]));
    assert!(
        s.contains("downloaded"),
        "a file at or past the published size is usable:\n{s}"
    );
    assert!(
        !s.contains("partial"),
        "and must not still be offered as resumable:\n{s}"
    );

    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn exactly_one_catalogue_entry_is_marked_the_default() {
    // The marker is how a user picks without reading four descriptions, so
    // it is worth nothing if it is on all of them or none. Inverting the
    // comparison moves it onto every entry but the right one.
    let d = repo("model-default");
    let data = scratch("model-default-data");
    std::fs::create_dir_all(models_dir_in(&data)).unwrap();

    let s = stdout(&run_with_data_dir(&d, &data, &["model"]));
    assert_eq!(
        s.matches("(default)").count(),
        1,
        "exactly one entry carries the marker:\n{s}"
    );
    assert!(
        s.contains(&format!("{} (default)", trellis_model::DEFAULT)),
        "and it is the one the catalogue names as the default:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_model_listing_says_where_colibri_weights_come_from() {
    // ADR 0047 decision 11: the catalogue holds one pinned GGUF file per
    // entry, so colibri's multi-shard containers are not in it. Someone
    // looking for them in `trellis model` would otherwise find nothing and
    // no hint, and might fetch shards by hand without the per-shard SHA-256
    // checks colibri's own tooling makes. One line, so the listing stays a
    // listing.
    let d = repo("model-runtime-note");
    let data = scratch("model-runtime-note-data");
    std::fs::create_dir_all(models_dir_in(&data)).unwrap();

    let s = stdout(&run_with_data_dir(&d, &data, &["model"]));
    let lines: Vec<&str> = s.lines().filter(|l| l.contains("colibri")).collect();
    assert_eq!(lines.len(), 1, "exactly one line names colibri:\n{s}");
    let line = lines[0];
    for needle in [
        "not in this catalogue",
        "coli convert",
        "coli mirror verify",
        "docs/runtimes/colibri.md",
    ] {
        assert!(
            line.contains(needle),
            "the colibri line must say `{needle}`:\n{line}"
        );
    }
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn model_pull_colibri_2_0_0_families_get_colibris_tooling_too() {
    // #612: the list stopped at colibri 1.12.1's nine families, so
    // `model pull "MiMo-V2.6 Flash"`, a family 2.0.0 serves, got the generic
    // catalogue error, which reads as a typo of a GGUF entry. Matched as the
    // registry spells it, in any case.
    let d = repo("model-pull-colibri-2");
    let data = scratch("model-pull-colibri-2-data");
    std::fs::create_dir_all(models_dir_in(&data)).unwrap();
    for name in ["MiMo-V2.6 Flash", "qwen3-coder-30b-a3b"] {
        let out = run_with_data_dir(&d, &data, &["model", "pull", name]);
        assert!(
            !out.status.success(),
            "{name}: a colibri family must not pull"
        );
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            err.contains("is a colibri family") && err.contains("coli convert"),
            "{name}: {err}"
        );
        assert!(
            !err.contains("Known:"),
            "{name}: not the generic error:\n{err}"
        );
    }
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn pull_a_colibri_family_names_colibris_tooling_and_other_names_keep_the_generic_error() {
    // ADR 0047 decision 11: `model pull` does not fetch colibri containers.
    // Someone who types a colibri family there should learn where its
    // weights come from, not read a catalogue list that never had it; and a
    // name that is neither must still get the error that lists the
    // catalogue, so a typo of a catalogue entry is not sent to colibri.
    let d = repo("model-pull-colibri");
    let data = scratch("model-pull-colibri-data");
    std::fs::create_dir_all(models_dir_in(&data)).unwrap();

    let out = run_with_data_dir(&d, &data, &["model", "pull", "Kimi K3"]);
    assert!(!out.status.success(), "a colibri family must not pull");
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    let lines: Vec<&str> = err.lines().filter(|l| l.contains("colibri")).collect();
    assert_eq!(lines.len(), 1, "exactly one line names colibri:\n{err}");
    for needle in [
        "coli convert",
        "coli mirror verify",
        "docs/runtimes/colibri.md",
    ] {
        assert!(
            lines[0].contains(needle),
            "the refusal must say `{needle}`:\n{err}"
        );
    }
    assert!(!err.contains("Known:"), "not the generic error:\n{err}");

    let out = run_with_data_dir(&d, &data, &["model", "pull", "no-such-model"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        err.contains("no model named `no-such-model`") && err.contains("Known:"),
        "an unrelated name keeps the generic error:\n{err}"
    );
    assert!(
        !err.contains("colibri"),
        "and is not sent to colibri:\n{err}"
    );

    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn model_path_names_a_real_location_under_the_data_directory() {
    // `trellis model path` exists so a user can put the file there
    // themselves — over a link the downloader cannot use, or from a copy on
    // another machine. A path that is empty, or that is not where the tool
    // will actually look, sends them to do the work twice.
    //
    // Both `models_dir` and `weights_path` can be replaced with a default
    // `PathBuf`, which is the empty one, and the command still prints a line
    // and exits zero.
    let d = repo("model-path");
    let data = scratch("model-path-data");

    let s = stdout(&run_with_data_dir(&d, &data, &["model", "path"]));
    let printed = s.trim();
    assert!(
        !printed.is_empty(),
        "an empty path is not an answer:\n{s:?}"
    );

    let expected = models_dir_in(&data).join(
        trellis_model::catalog::find(trellis_model::DEFAULT)
            .expect("the default is in the catalogue")
            .file,
    );
    assert_eq!(
        Path::new(printed),
        expected,
        "the path printed must be the one `model` itself looks in, or a \
         user who puts the file there finds it was not enough"
    );
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn pull_stops_before_the_network_when_the_weights_are_already_here() {
    // The branch of `pull` worth the most: it is what stops a re-run of
    // `trellis model pull` from re-downloading a file that is already
    // complete. Inverting the comparison turns the guard into its opposite
    // and every repeat invocation becomes a fresh download.
    //
    // Safe precisely because it returns *before* curl.
    let d = repo("model-pull-present");
    let data = scratch("model-pull-data");
    let dir = models_dir_in(&data);
    std::fs::create_dir_all(&dir).unwrap();

    let entry = trellis_model::catalog::find(trellis_model::DEFAULT).unwrap();
    let f = std::fs::File::create(dir.join(entry.file)).unwrap();
    f.set_len(entry.bytes).unwrap();
    drop(f);

    let out = run_with_data_dir(&d, &data, &["model", "pull"]);
    let s = combined(&out);
    assert!(out.status.success(), "{s}");
    assert!(
        s.contains("is already here"),
        "a complete file must end the command, not start a download:\n{s}"
    );
    assert!(
        !s.contains("from https://"),
        "and it must not even name the URL, because nothing is being \
         fetched:\n{s}"
    );

    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn pull_says_it_is_resuming_only_when_a_partial_download_is_staged() {
    // #248: on a slow link "resuming at" versus a silent restart is the
    // difference between hours, and a line that claims a resume with
    // nothing staged is as wrong as one missing. Neither case reaches
    // curl: a file one byte longer than the pin is refused as too long
    // before any fetch, whether it is staged or at the final name (which
    // `pull` moves to the staging name first, after the line is printed).
    let entry = trellis_model::catalog::find(trellis_model::DEFAULT).unwrap();
    let d = repo("model-pull-resume");
    for (staged, data) in [
        (true, scratch("model-pull-resume-staged")),
        (false, scratch("model-pull-resume-placed")),
    ] {
        let dir = models_dir_in(&data);
        std::fs::create_dir_all(&dir).unwrap();
        let name = if staged {
            trellis_model::pull::partial_name(entry.file)
        } else {
            entry.file.to_string()
        };
        let f = std::fs::File::create(dir.join(name)).unwrap();
        f.set_len(entry.bytes + 1).unwrap();
        drop(f);

        let out = run_with_data_dir(&d, &data, &["model", "pull"]);
        let s = combined(&out);
        assert!(!out.status.success(), "{s}");
        assert!(s.contains("from https://"), "{s}");
        assert!(s.contains("the pinned file is"), "{s}");
        assert_eq!(
            stdout(&out).contains("resuming at"),
            staged,
            "staged: {staged}\n{s}"
        );
        let _ = std::fs::remove_dir_all(&data);
    }
    let _ = std::fs::remove_dir_all(&d);
}
