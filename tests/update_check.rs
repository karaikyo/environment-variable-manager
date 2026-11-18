use environment_variable_manager::update_check::*;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
struct Replies(Mutex<Vec<HttpResponse>>);
impl ReleaseTransport for Replies {
    fn fetch(&self, _: &str, _: &CheckControl) -> Result<HttpResponse, String> {
        Ok(self.0.lock().unwrap().remove(0))
    }
}
fn release(version: &str) -> String {
    format!(
        r#"{{"tag_name":"v{version}","draft":false,"prerelease":true,"published_at":"2026-11-17T00:00:00Z","assets":[{{"name":"environment-variable-manager-{version}-windows-x64.zip","state":"uploaded"}}]}}"#
    )
}
fn response(body: String) -> HttpResponse {
    HttpResponse {
        status: 200,
        body: body.into_bytes(),
        link: None,
    }
}
fn finish(checker: &mut UpdateChecker) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while checker.is_running() {
        checker.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn manual_check_finds_newest_numeric_preview_and_constructs_project_url() {
    let transport = Arc::new(Replies(Mutex::new(vec![response(format!(
        "[{},{},{}]",
        release("0.1.9"),
        release("0.1.10"),
        release("0.1.3")
    ))])));
    let mut checker = UpdateChecker::new(transport.clone());
    assert_eq!(checker.status(), &UpdateStatus::Idle);
    assert_eq!(
        transport.0.lock().unwrap().len(),
        1,
        "No request before explicit start"
    );
    assert!(checker.start());
    finish(&mut checker);
    let UpdateStatus::Available(found) = checker.status() else {
        panic!("Expected newer release: {:?}", checker.status())
    };
    assert_eq!(found.version(), "0.1.10");
    assert_eq!(
        found.url(),
        "https://github.com/karaikyo/environment-variable-manager/releases/tag/v0.1.10"
    );
}

fn check_responses(responses: Vec<HttpResponse>) -> UpdateStatus {
    let mut checker = UpdateChecker::new(Arc::new(Replies(Mutex::new(responses))));
    assert!(checker.start());
    finish(&mut checker);
    checker.status().clone()
}
fn failed(responses: Vec<HttpResponse>) {
    assert!(matches!(
        check_responses(responses),
        UpdateStatus::Failed(_)
    ));
}
#[test]
fn equal_and_older_usable_releases_never_offer_a_downgrade() {
    for version in [CURRENT_VERSION, "0.0.1"] {
        assert_eq!(
            check_responses(vec![response(format!("[{}]", release(version)))]),
            UpdateStatus::UpToDate
        );
    }
}
#[test]
fn excludes_drafts_unpublished_tags_and_missing_assets_but_includes_preview() {
    let ignored = [
        release("9.0.0").replace("\"draft\":false", "\"draft\":true"),
        release("8.0.0").replace("\"2026-11-17T00:00:00Z\"", "null"),
        release("7.0.0").replace("v7.0.0", "nightly"),
        release("6.0.0").replace("windows-x64.zip", "windows-arm64.zip"),
        release("5.0.0").replace("uploaded", "new"),
        release("4.0.0").replace("v4.0.0", "v4.0.0-beta"),
        release("3.0.0").replace("v3.0.0", r"v3.0.0\u0000"),
        release("2.0.0").replace("v2.0.0", "v4294967296.0.0"),
    ];
    let status = check_responses(vec![response(format!(
        "[{},{}]",
        ignored.join(","),
        release("1.0.0")
    ))]);
    let UpdateStatus::Available(found) = status else {
        panic!("Expected eligible preview")
    };
    assert_eq!(found.version(), "1.0.0");
}
#[test]
fn no_usable_release_is_failure_not_false_up_to_date() {
    for body in [
        "[]".into(),
        format!("[{}]", release("1.0.0").replace("uploaded", "new")),
    ] {
        failed(vec![response(body)]);
    }
}
#[test]
fn malformed_or_ambiguous_metadata_never_becomes_up_to_date() {
    let original = release("1.0.0");
    for body in [
        "null".into(), "{}".into(), "[1]".into(), "[null]".into(), "[".into(),
        format!("[{}]",original.replace("\"draft\":false","\"draft\":\"false\"")),
        format!("[{}]",original.replace("\"prerelease\":true","\"prerelease\":null")),
        format!("[{}]",original.replace("\"draft\":false","\"draft\":false,\"draft\":true")),
        format!("[{}]",original.replace("\"state\":\"uploaded\"","\"state\":\"uploaded\",\"state\":\"new\"")),
        format!("[{}]",original.replace("\"assets\":[", "\"assets\":{\"x\":")),
        format!("[{}]",original.replace("\"tag_name\":", "\"html_url\":\"https://evil.invalid/\",\"tag_name\":")),
        format!("[{}]",original.replace("\"tag_name\":", "\"html_url\":\"file:///C:/payload.exe\",\"tag_name\":")),
        format!("[{}]",original.replace("\"tag_name\":", "\"html_url\":\"https://github.com/karaikyo/environment-variable-manager/releases/tag/v1.0.0\\u0000\",\"tag_name\":")),
    ] { failed(vec![response(body)]); }
}
#[test]
fn newest_can_be_on_later_page_and_order_does_not_determine_version() {
    let mut first = response(format!("[{}]", release("0.1.5")));
    first.link = Some(format!(
        "<{}>; rel=\"next\", <{}>; rel=\"last\"",
        RELEASES_URL.replace("&page=1", "&page=2"),
        RELEASES_URL.replace("&page=1", "&page=2")
    ));
    let result = check_responses(vec![first, response(format!("[{}]", release("0.2.0")))]);
    let UpdateStatus::Available(found) = result else {
        panic!("Expected later release")
    };
    assert_eq!(found.version(), "0.2.0");
}
#[test]
fn pagination_rejects_cycles_foreign_hosts_and_unbounded_pages() {
    for link in [
        format!("<{RELEASES_URL}>; rel=\"next\""),
        "<https://api.github.com.evil.invalid/repos/x/releases?page=2&per_page=100>; rel=\"next\"".into(),
        "<http://api.github.com/repos/karaikyo/environment-variable-manager/releases?page=2&per_page=100>; rel=\"next\"".into(),
        "<https://api.github.com/repos/other/repository/releases?page=2&per_page=100>; rel=\"next\"".into(),
        "<https://api.github.com/repos/karaikyo/environment-variable-manager/releases?page=2&per_page=100&token=x>; rel=\"next\"".into(),
        "malformed".into(),
    ] { let mut page=response(format!("[{}]",release("1.0.0")));page.link=Some(link);failed(vec![page]); }
    let mut pages = Vec::new();
    for page in 1..=5 {
        let mut reply = response(format!("[{}]", release("1.0.0")));
        reply.link = Some(format!(
            "<{}>; rel=\"next\"",
            RELEASES_URL.replace("&page=1", &format!("&page={}", page + 1))
        ));
        pages.push(reply);
    }
    failed(pages);
}
#[test]
fn failures_and_size_limits_are_not_success_results() {
    for status in [301, 403, 404, 429, 500] {
        let mut reply = response(format!("[{}]", release("1.0.0")));
        reply.status = status;
        failed(vec![reply]);
    }
    failed(vec![HttpResponse {
        status: 200,
        body: vec![b' '; MAX_RESPONSE_BYTES + 1],
        link: None,
    }]);
    failed(vec![HttpResponse {
        status: 200,
        body: vec![255],
        link: None,
    }]);
    let mut replies = Vec::new();
    for page in 1..=3 {
        let mut body = format!("[{}]", release("1.0.0"));
        body.push_str(&" ".repeat(MAX_RESPONSE_BYTES - body.len()));
        let mut reply = response(body);
        if page < 3 {
            reply.link = Some(format!(
                "<{}>; rel=\"next\"",
                RELEASES_URL.replace("&page=1", &format!("&page={}", page + 1))
            ));
        }
        replies.push(reply);
    }
    failed(replies);
}
struct Delayed(std::sync::atomic::AtomicUsize);
impl ReleaseTransport for Delayed {
    fn fetch(&self, _: &str, control: &CheckControl) -> Result<HttpResponse, String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        loop {
            control.check()?;
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
#[test]
fn one_explicit_check_can_be_cancelled_and_retried_without_concurrent_requests() {
    let transport = Arc::new(Delayed(std::sync::atomic::AtomicUsize::new(0)));
    let mut checker = UpdateChecker::new(transport.clone());
    assert!(checker.start());
    assert!(!checker.start());
    assert_eq!(checker.status(), &UpdateStatus::Checking);
    checker.cancel();
    finish(&mut checker);
    assert!(matches!(checker.status(), UpdateStatus::Failed(_)));
    assert!(checker.start());
    assert_eq!(checker.status(), &UpdateStatus::Checking);
    checker.cancel();
    finish(&mut checker);
    assert!(transport.0.load(std::sync::atomic::Ordering::SeqCst) <= 2);
}
#[test]
fn retries_clear_previous_release_and_connection_errors_are_visible() {
    struct Sequence(Mutex<usize>);
    impl ReleaseTransport for Sequence {
        fn fetch(&self, _: &str, _: &CheckControl) -> Result<HttpResponse, String> {
            let mut n = self.0.lock().unwrap();
            *n += 1;
            if *n == 1 {
                Ok(response(format!("[{}]", release("1.0.0"))))
            } else {
                Err("Connection failed; try again".into())
            }
        }
    }
    let mut checker = UpdateChecker::new(Arc::new(Sequence(Mutex::new(0))));
    assert!(checker.start());
    finish(&mut checker);
    assert!(matches!(checker.status(), UpdateStatus::Available(_)));
    assert!(checker.start());
    assert_eq!(checker.status(), &UpdateStatus::Checking);
    finish(&mut checker);
    assert_eq!(
        checker.status(),
        &UpdateStatus::Failed("Connection failed; try again".into())
    );
}
