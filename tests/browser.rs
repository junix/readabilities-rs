#![cfg(feature = "browser")]

use readabilities_rs::{
    Acquisition, BrowserPolicy, ErrorKind, ExecutionOutcome, ReadRequest, Reader,
};
use url::Url;

#[tokio::test]
async fn browser_launch_budget_is_checked_before_chrome_starts() {
    let mut request = ReadRequest::url(Url::parse("http://127.0.0.1:9/article").unwrap());
    request.url_policy.allow_private_networks = true;
    request.url_policy.budget.max_browser_launches = 0;
    request.url_policy.acquisition = Acquisition::Browser(BrowserPolicy::default());

    let execution = Reader::new().execute(request).await;
    assert!(matches!(
        execution.outcome,
        ExecutionOutcome::Failure(readabilities_rs::ReadError {
            kind: ErrorKind::BudgetExceeded,
            ..
        })
    ));
    assert_eq!(execution.cost.browser_launches, 0);
}
