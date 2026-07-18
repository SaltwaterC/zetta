use super::*;

fn prompt_with(query: &str, commands: &[&str], ssh_hosts: &[&str]) -> MultiCommandPrompt {
    let catalog = CompletionCatalog::new(
        commands.iter().map(|value| (*value).to_owned()).collect(),
        ssh_hosts.iter().map(|value| (*value).to_owned()).collect(),
    );
    let mut prompt = MultiCommandPrompt::new(catalog);
    prompt.query = query.to_owned();
    prompt.cursor = query.len();
    prompt.home = PathBuf::from("/home/test");
    prompt
}

fn apply_ready_completion(prompt: &mut MultiCommandPrompt, reverse: bool) -> CompletionRequest {
    let mut request = prompt.begin_completion_request(PathBuf::from("/work"), reverse);
    let CompletionSource::Ready(candidates) = request.take_source() else {
        panic!("expected an in-memory completion request");
    };
    assert!(prompt.apply_completion_result(&request, candidates));
    request
}

#[test]
fn expands_a_brace_list_inside_an_argument() {
    assert_eq!(
        expand_multi_command("ssh {{a,b,c,d}}.example.com", 64).unwrap(),
        [
            "ssh a.example.com",
            "ssh b.example.com",
            "ssh c.example.com",
            "ssh d.example.com",
        ]
    );
}

#[test]
fn expands_multiple_and_nested_brace_lists() {
    assert_eq!(
        expand_multi_command("echo {{dev,prod}}-{{a,{{b,c}}}}", 64).unwrap(),
        [
            "echo dev-a",
            "echo dev-b",
            "echo dev-c",
            "echo prod-a",
            "echo prod-b",
            "echo prod-c",
        ]
    );
}

#[test]
fn leaves_single_quoted_and_escaped_braces_for_the_shell() {
    assert_eq!(
        expand_multi_command(r#"echo {shell,brace} '{{x,y}}' \{{a,b}} {{c,d}}"#, 64).unwrap(),
        [
            r#"echo {shell,brace} '{{x,y}}' \{{a,b}} c"#,
            r#"echo {shell,brace} '{{x,y}}' \{{a,b}} d"#,
        ]
    );
}

#[test]
fn requires_an_expandable_list() {
    assert!(expand_multi_command("ssh example.com", 64).is_err());
    assert!(expand_multi_command("echo {only}", 64).is_err());
    assert!(expand_multi_command("echo {a,b}", 64).is_err());
}

#[test]
fn rejects_expansions_over_the_pane_limit() {
    assert_eq!(
        expand_multi_command("echo {{a,b}}-{{1,2}}", 3).unwrap_err(),
        "A multi-command can create at most 3 panes"
    );
}

#[test]
fn malformed_openers_are_scanned_once_before_a_valid_group() {
    let malformed_prefix = "{{".repeat(8_192);
    let template = format!("{malformed_prefix}{{{{a,b}}}}");

    let (start, _, alternatives) = first_double_brace_list(&template).unwrap();
    assert_eq!(start, malformed_prefix.len());
    assert_eq!(alternatives, ["a", "b"]);
}

#[test]
fn parses_ssh_config_host_aliases_without_wildcards() {
    let hosts = parse_ssh_config_hosts(
        r#"
            Host production prod
                HostName prod.example.com
            host Staging # comments are ignored
            Host *
            Host !blocked web-?
            Host PROD
        "#,
    );

    assert_eq!(hosts, ["prod", "production", "Staging"]);
}

#[test]
fn ssh_alias_completion_works_inside_a_multi_command_group() {
    let mut prompt = prompt_with("ssh {{pro", &[], &["production", "staging"]);
    let request = apply_ready_completion(&mut prompt, false);

    assert_eq!(&request.query[..request.start], "ssh {{");
    assert_eq!(prompt.query, "ssh {{production");
}

#[test]
fn ssh_alias_completion_preserves_an_explicit_user() {
    let mut prompt = prompt_with("ssh admin@pro", &[], &["production", "staging"]);
    apply_ready_completion(&mut prompt, false);

    assert_eq!(prompt.query, "ssh admin@production");
}

#[test]
fn loads_ssh_aliases_from_a_config_file() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config");
    fs::write(&config, "Host alpha beta\n  HostName example.com\n").unwrap();

    assert_eq!(ssh_config_hosts(&config), ["alpha", "beta"]);
}

#[test]
fn tab_cycles_prompt_native_completions_in_both_directions() {
    let mut prompt = prompt_with("ca", &["cargo", "cat"], &[]);

    apply_ready_completion(&mut prompt, false);
    assert_eq!(prompt.query, "cargo");
    assert!(prompt.cycle_existing_completion(false));
    assert_eq!(prompt.query, "cat");
    assert!(prompt.cycle_existing_completion(true));
    assert_eq!(prompt.query, "cargo");
}

#[test]
fn completion_navigation_applies_candidates_and_wraps() {
    let mut prompt = prompt_with("ca", &[], &[]);
    prompt.completion_candidates = vec!["cargo".into(), "cat".into()];
    prompt.completion_start = 0;
    prompt.completion_end = 2;

    prompt.navigate_completion(false);
    assert_eq!(prompt.completion_selected, Some(0));
    assert_eq!(prompt.query, "cargo");
    prompt.navigate_completion(false);
    assert_eq!(prompt.completion_selected, Some(1));
    assert_eq!(prompt.query, "cat");
    prompt.navigate_completion(false);
    assert_eq!(prompt.completion_selected, Some(0));
    assert_eq!(prompt.query, "cargo");
    prompt.navigate_completion(true);
    assert_eq!(prompt.completion_selected, Some(1));
    assert_eq!(prompt.query, "cat");
}

#[test]
fn selecting_a_completion_ignores_out_of_range_rows() {
    let mut prompt = prompt_with("ca", &[], &[]);
    prompt.completion_candidates = vec!["cargo".into()];
    prompt.completion_start = 0;
    prompt.completion_end = 2;

    prompt.select_completion(99);
    assert_eq!(prompt.query, "ca");
    assert_eq!(prompt.completion_selected, None);
}

#[test]
fn enter_accepts_the_completion_layer_before_command_submission() {
    let mut prompt = prompt_with("cargo", &[], &[]);
    prompt.completion_candidates = vec!["cargo".into(), "cat".into()];
    prompt.completion_selected = Some(0);
    prompt.completion_start = 0;
    prompt.completion_end = 5;
    prompt.completion_add_space = true;

    assert!(prompt.accept_completion());
    assert_eq!(prompt.query, "cargo ");
    assert!(prompt.completion_candidates.is_empty());
    assert_eq!(prompt.completion_selected, None);
    assert!(!prompt.accept_completion());
}

#[test]
fn accepting_a_double_brace_completion_does_not_add_space() {
    let mut prompt = prompt_with("ssh {{production, sta", &[], &["staging"]);

    apply_ready_completion(&mut prompt, false);
    assert_eq!(prompt.query, "ssh {{production, staging");
    assert!(prompt.accept_completion());
    assert_eq!(prompt.query, "ssh {{production, staging");
}

#[test]
fn accepting_a_completion_does_not_duplicate_existing_whitespace() {
    let mut prompt = prompt_with("cargo --version", &[], &[]);
    prompt.cursor = 5;
    prompt.completion_candidates = vec!["cargo".into()];
    prompt.completion_selected = Some(0);
    prompt.completion_start = 0;
    prompt.completion_end = 5;
    prompt.completion_add_space = true;

    assert!(prompt.accept_completion());
    assert_eq!(prompt.query, "cargo --version");
}

#[test]
fn filesystem_completion_uses_the_active_working_directory() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("source")).unwrap();
    fs::write(directory.path().join("script.txt"), "").unwrap();
    fs::write(directory.path().join("unrelated.txt"), "").unwrap();

    let candidates = filesystem_candidates("s", directory.path(), directory.path());

    assert_eq!(
        candidates,
        [
            "script.txt".to_owned(),
            format!("source{}", std::path::MAIN_SEPARATOR)
        ]
    );
}

#[cfg(unix)]
#[test]
fn filesystem_completion_marks_symlinked_directories_without_statting_regular_entries() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("target")).unwrap();
    symlink(
        directory.path().join("target"),
        directory.path().join("linked"),
    )
    .unwrap();

    let candidates = filesystem_candidates("lin", directory.path(), directory.path());

    assert_eq!(candidates, [format!("linked{}", std::path::MAIN_SEPARATOR)]);
}

#[test]
fn stale_completion_results_are_rejected() {
    let mut prompt = prompt_with("cat s", &[], &[]);
    let request = prompt.begin_completion_request(PathBuf::from("/work"), false);
    assert!(completion_request_is_current(Some(&prompt), &request));

    prompt.query.push('x');
    prompt.cursor += 1;
    prompt.clear_completion();

    assert!(!completion_request_is_current(Some(&prompt), &request));
    assert!(!prompt.apply_completion_result(&request, vec!["stale".into()]));
    assert_eq!(prompt.query, "cat sx");
}

#[test]
fn filesystem_completion_is_returned_as_deferred_work() {
    let mut prompt = prompt_with("cat s", &[], &[]);
    let request = prompt.begin_completion_request(PathBuf::from("/work"), false);

    assert!(prompt.completion_loading);
    assert!(matches!(
        request.source,
        CompletionSource::Filesystem {
            prefix,
            working_directory,
            ..
        } if prefix == "s" && working_directory == Path::new("/work")
    ));
}

#[test]
fn normalized_catalog_matching_is_case_insensitive_without_changing_display() {
    let catalog = CompletionCatalog::new(Vec::new(), vec!["Production-EU".to_owned()]);
    let mut prompt = MultiCommandPrompt::new(catalog);
    prompt.query = "ssh production".to_owned();
    prompt.cursor = prompt.query.len();

    apply_ready_completion(&mut prompt, false);
    assert_eq!(prompt.query, "ssh Production-EU");
}

#[test]
fn catalog_matching_is_bounded_before_candidates_are_collected() {
    let catalog = CompletionCatalog::new(
        ["alpha", "alpine", "also", "alto"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        Vec::new(),
    );

    assert_eq!(
        prefix_matches_entries_with_limit(&catalog.commands, "al", false, 2),
        ["alpha", "alpine"]
    );
}

#[test]
fn completion_task_cancellation_drops_the_active_work() {
    struct DropMarker<'a>(&'a AtomicBool);
    impl Drop for DropMarker<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let dropped = AtomicBool::new(false);
    let mut task = Some(DropMarker(&dropped));
    cancel_completion_task(&mut task);

    assert!(task.is_none());
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn completion_catalog_cache_is_single_flight_across_threads() {
    let cache = OnceLock::new();
    let loads = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let handles = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    cached_completion_catalog(&cache, || {
                        loads.fetch_add(1, Ordering::SeqCst);
                        CompletionCatalog::new(vec!["cargo".to_owned()], Vec::new())
                    })
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            assert_eq!(handle.join().unwrap().commands.len(), 1);
        }
    });
    assert_eq!(loads.load(Ordering::SeqCst), 1);
}

#[test]
fn bounded_candidates_are_sorted_and_keep_the_smallest_results() {
    let candidates = bounded_sorted_candidates(["delta", "alpha", "charlie", "bravo"], 3);
    assert_eq!(candidates, ["alpha", "bravo", "charlie"]);
}

#[test]
fn bounded_unique_candidates_do_not_spend_capacity_on_duplicates() {
    let candidates =
        bounded_sorted_unique_candidates(["delta", "alpha", "alpha", "charlie", "bravo"], 3);
    assert_eq!(candidates, ["alpha", "bravo", "charlie"]);
}

#[test]
fn cancellation_stops_candidate_iteration_before_more_work_is_pulled() {
    let cancellation = CompletionCancellation::default();
    let iterator_cancellation = cancellation.clone();
    let pulled = AtomicUsize::new(0);
    let candidates = std::iter::from_fn(|| {
        let value = pulled.fetch_add(1, Ordering::SeqCst);
        if value == 2 {
            iterator_cancellation.cancel();
        }
        Some(value)
    });

    assert_eq!(
        bounded_sorted_candidates_cancellable(candidates, 8, &cancellation),
        [0, 1, 2]
    );
    assert_eq!(pulled.load(Ordering::SeqCst), 3);
}

#[test]
fn rendered_query_parts_are_reused_until_the_query_changes() {
    let mut prompt = prompt_with(
        "a-command-long-enough-to-use-shared-storage --argument-long-enough-to-be-shared",
        &[],
        &[],
    );
    prompt.cursor = "a-command-long-enough-to-use-shared-storage".len();

    let (before, after) = prompt.rendered_query_parts();
    let (cached_before, cached_after) = prompt.rendered_query_parts();
    assert_eq!(before, cached_before);
    assert_eq!(after, cached_after);
    assert_eq!(before.as_ptr(), cached_before.as_ptr());
    assert_eq!(after.as_ptr(), cached_after.as_ptr());

    prompt.query.push_str(" --changed");
    prompt.cursor = prompt.query.len();
    prompt.mark_query_changed();
    let (changed_before, changed_after) = prompt.rendered_query_parts();
    assert_eq!(changed_before, prompt.query);
    assert_eq!(changed_after, "");
}

#[test]
fn oversized_templates_are_rejected_before_expansion() {
    let template = format!(
        "{}{{{{a,b}}}}",
        "x".repeat(MAX_MULTI_COMMAND_TEMPLATE_BYTES)
    );

    assert!(
        expand_multi_command(&template, 64)
            .unwrap_err()
            .contains("at most 65536 bytes")
    );
}

#[test]
fn many_brace_groups_are_rejected_without_recursive_expansion() {
    let template = "{{a,b}}".repeat(4_096);

    assert_eq!(
        expand_multi_command(&template, 64).unwrap_err(),
        "A multi-command can create at most 64 panes"
    );
}
