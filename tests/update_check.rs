use environment_variable_manager::update_check::*;
use std::{sync::{Arc, Mutex}, time::{Duration, Instant}};
struct Replies(Mutex<Vec<HttpResponse>>);
impl ReleaseTransport for Replies {
    fn fetch(&self, _: &str, _: &CheckControl) -> Result<HttpResponse,String> {
        Ok(self.0.lock().unwrap().remove(0))
    }
}
fn release(version: &str) -> String {
    format!(r#"{{"tag_name":"v{version}","draft":false,"prerelease":true,"published_at":"2026-11-17T00:00:00Z","assets":[{{"name":"environment-variable-manager-{version}-windows-x64.zip","state":"uploaded"}}]}}"#)
}
fn response(body: String) -> HttpResponse { HttpResponse {status:200,body:body.into_bytes(),link:None} }
fn finish(checker: &mut UpdateChecker) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while checker.is_running() {
        checker.poll(); assert!(Instant::now()<deadline); std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn manual_check_finds_newest_numeric_preview_and_constructs_project_url() {
    let transport = Arc::new(Replies(Mutex::new(vec![response(format!("[{},{},{}]",release("0.1.9"),release("0.1.10"),release("0.1.3")))])));
    let mut checker = UpdateChecker::new(transport.clone());
    assert_eq!(checker.status(), &UpdateStatus::Idle);
    assert_eq!(transport.0.lock().unwrap().len(),1,"No request before explicit start");
    assert!(checker.start()); finish(&mut checker);
    let UpdateStatus::Available(found) = checker.status() else { panic!("Expected newer release: {:?}",checker.status()) };
    assert_eq!(found.version(),"0.1.10");
    assert_eq!(found.url(),"https://github.com/karaikyo/environment-variable-manager/releases/tag/v0.1.10");
}
