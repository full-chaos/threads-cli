use threads_core::{Error, PermissionRequirement};

use super::*;

#[test]
fn audience_refresh_error_names_threads_basic_for_account_lookup_permission() {
    let error = audience_refresh_error(Error::MissingPermission {
        requirement: PermissionRequirement::AuthenticatedAccount,
        detail: "403 Forbidden".into(),
    });

    assert!(error.to_string().contains("threads_basic"));
    assert!(error.to_string().contains("threads-cli auth login"));
}

#[test]
fn audience_refresh_error_names_insights_for_insight_permission() {
    let error = audience_refresh_error(Error::MissingPermission {
        requirement: PermissionRequirement::AudienceInsights,
        detail: "403 Forbidden".into(),
    });

    assert!(error.to_string().contains("threads_manage_insights"));
    assert!(error.to_string().contains("threads-cli auth login"));
}

#[test]
fn audience_refresh_error_preserves_non_permission_errors() {
    let error = audience_refresh_error(Error::Network("offline".into()));

    assert_eq!(
        error.to_string(),
        "refresh audience: network error: offline"
    );
}
