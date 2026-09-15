// WORKSPACE-PATH:allow-fixture: fixtures name a foreign checkout root, drive, or home directory to exercise path handling, so the literal is the value under assertion rather than a binding this build resolves
use sdkwork_agent_kernel::{
    HostEnvPolicy, KernelError, KernelResult, ProviderHealth, SideEffectLevel,
};
use sdkwork_code_kernel::{
    CodeKernelCapability, CommandResult, GeneratedFilePolicy, PatchOperation, PatchProvider,
    PatchSet, ReviewFinding, ReviewSeverity, TerminalCommand, VcsProvider, VcsSnapshot,
    VerificationPlan, VerificationReport, Workspace, WorkspaceFile, WorkspaceProvider,
    CODE_CAPABILITY_MANIFEST_SCHEMA, CODE_CONFORMANCE_REPORT_SCHEMA,
    CODE_RUNTIME_DIAGNOSTICS_SCHEMA,
};

#[test]
fn code_kernel_capabilities_are_namespaced_and_provider_neutral() {
    assert_eq!(
        CodeKernelCapability::WorkspaceList.as_str(),
        "code.workspace.list"
    );
    assert_eq!(
        CodeKernelCapability::WorkspaceRead.as_str(),
        "code.workspace.read"
    );
    assert_eq!(
        CodeKernelCapability::WorkspaceWrite.as_str(),
        "code.workspace.write"
    );
    assert_eq!(CodeKernelCapability::VcsDiff.as_str(), "code.vcs.diff");
    assert_eq!(
        CodeKernelCapability::VcsRestore.as_str(),
        "code.vcs.restore"
    );
    assert_eq!(
        CodeKernelCapability::PatchApply.as_str(),
        "code.patch.apply"
    );
    assert_eq!(
        CodeKernelCapability::PatchRollback.as_str(),
        "code.patch.rollback"
    );
    assert_eq!(
        CodeKernelCapability::TerminalRun.as_str(),
        "code.terminal.run"
    );
    assert_eq!(
        CodeKernelCapability::KnowledgeList.as_str(),
        "code.knowledge.list"
    );
}

#[test]
fn code_kernel_schema_constants_expose_machine_readable_standard_contracts() {
    assert!(CODE_CAPABILITY_MANIFEST_SCHEMA.contains("SDKWork Code Kernel Capability Manifest"));
    assert!(CODE_RUNTIME_DIAGNOSTICS_SCHEMA.contains("SDKWork Code Kernel Runtime Diagnostics"));
    assert!(CODE_RUNTIME_DIAGNOSTICS_SCHEMA.contains("code_runtime_diagnostics"));
    assert!(CODE_CONFORMANCE_REPORT_SCHEMA.contains("SDKWork Code Kernel Conformance Report"));
}

#[test]
fn workspace_preserves_root_trust_and_generated_file_policy() {
    let workspace = Workspace::new("workspace.1", "D:/repo")
        .with_trust_level("trusted")
        .with_generated_file_policy(GeneratedFilePolicy::Protected);

    assert_eq!(workspace.workspace_id, "workspace.1");
    assert_eq!(workspace.root, "D:/repo");
    assert_eq!(workspace.trust_level, "trusted");
    assert_eq!(
        workspace.generated_file_policy,
        GeneratedFilePolicy::Protected
    );
}

#[test]
fn workspace_provider_trait_supports_deterministic_fake_workspace() {
    let provider = FakeWorkspaceProvider;
    let workspace = Workspace::new("workspace.1", "workspace");

    let file = provider
        .read_file(&workspace, "src/lib.rs")
        .expect("file reads");
    assert_eq!(file.path, "src/lib.rs");
    assert_eq!(file.content.as_deref(), Some("pub fn main() {}"));
    assert_eq!(provider.health(), ProviderHealth::available());

    let denied = provider.read_file(&workspace, "../secret.txt");
    assert_eq!(
        denied,
        Err(KernelError::PolicyDenied {
            reason_code: "code.workspace.path_denied".to_string()
        })
    );
}

#[test]
fn vcs_provider_trait_returns_normalized_snapshot() {
    let provider = FakeVcsProvider;
    let snapshot = provider
        .snapshot(&Workspace::new("workspace.1", "workspace"))
        .expect("snapshot loads");

    assert_eq!(snapshot.branch, "main");
    assert_eq!(snapshot.head_revision.as_deref(), Some("abc123"));
    assert!(snapshot.dirty);
    assert_eq!(snapshot.changed_files, ["src/lib.rs"]);
    assert_eq!(provider.health(), ProviderHealth::available());
}

#[test]
fn patch_set_requires_policy_for_side_effectful_application() {
    let patch = PatchSet::new("patch.1", "workspace.1", "Edit lib.rs")
        .add_operation(PatchOperation::update_file("src/lib.rs", "old", "new"))
        .with_policy_categories(vec!["code.patch.apply".to_string()]);

    assert_eq!(patch.side_effect_level(), SideEffectLevel::SideEffectful);
    assert!(patch.requires_policy());
    assert_eq!(patch.operations.len(), 1);
}

#[test]
fn patch_provider_validates_and_applies_patch_with_rollback_metadata() {
    let provider = FakePatchProvider;
    let workspace = Workspace::new("workspace.1", "workspace");
    let patch = PatchSet::new("patch.1", "workspace.1", "Edit lib.rs")
        .add_operation(PatchOperation::update_file("src/lib.rs", "old", "new"))
        .with_policy_categories(vec!["code.patch.apply".to_string()]);

    provider
        .validate_patch(&workspace, &patch)
        .expect("patch validates");
    let applied = provider
        .apply_patch(&workspace, patch)
        .expect("patch applies");

    assert_eq!(applied.status, "applied");
    assert_eq!(applied.rollback_token.as_deref(), Some("rollback.patch.1"));
    assert_eq!(provider.health(), ProviderHealth::available());
}

#[test]
fn workspace_vcs_and_patch_spi_cover_control_plane_operations() {
    let workspace = Workspace::new("workspace.1", "workspace");
    let workspace_provider = FakeWorkspaceProvider;

    let files = workspace_provider
        .list_files(&workspace, "src")
        .expect("workspace files list");
    assert_eq!(files[0].path, "src/lib.rs");
    assert!(files[0].is_file());

    let stat = workspace_provider
        .stat_file(&workspace, "src/lib.rs")
        .expect("workspace file stats");
    assert_eq!(stat.size_bytes, Some(16));

    let write_request =
        sdkwork_code_kernel::WorkspaceWriteRequest::new("src/lib.rs", "pub fn main() {}\n")
            .with_expected_version("abc123")
            .with_policy_categories(vec![CodeKernelCapability::WorkspaceWrite
                .as_str()
                .to_string()]);
    assert!(write_request.requires_policy());

    let write_result = workspace_provider
        .write_file(&workspace, write_request)
        .expect("workspace file writes");
    assert_eq!(write_result.version.as_deref(), Some("abc124"));

    let events = workspace_provider
        .watch_events(&workspace, Some("cursor.0"))
        .expect("workspace events load");
    assert_eq!(events[0].cursor, "cursor.1");

    let vcs_provider = FakeVcsProvider;
    let diff = vcs_provider
        .diff(
            &workspace,
            sdkwork_code_kernel::VcsDiffRequest::new().with_path("src/lib.rs"),
        )
        .expect("vcs diff loads");
    assert_eq!(
        diff.files[0].change_kind,
        sdkwork_code_kernel::VcsFileChangeKind::Modified
    );

    let blame = vcs_provider
        .blame(&workspace, "src/lib.rs")
        .expect("vcs blame loads");
    assert_eq!(blame[0].revision, "abc123");

    let commit = vcs_provider
        .commit_metadata(&workspace, "abc123")
        .expect("commit metadata loads");
    assert_eq!(commit.author, "SDKWork");

    let restore_request =
        sdkwork_code_kernel::VcsRestoreRequest::new(vec!["src/lib.rs".to_string()])
            .with_revision("HEAD")
            .with_policy_categories(vec![CodeKernelCapability::VcsRestore.as_str().to_string()]);
    assert!(restore_request.requires_policy());

    let restore = vcs_provider
        .restore(&workspace, restore_request)
        .expect("vcs restore reports");
    assert_eq!(restore.restored_paths, ["src/lib.rs"]);

    let patch_provider = FakePatchProvider;
    let patch = PatchSet::new("patch.1", "workspace.1", "Edit lib.rs")
        .add_operation(PatchOperation::update_file("src/lib.rs", "old", "new"));

    let preview = patch_provider
        .preview_patch(&workspace, &patch)
        .expect("patch preview loads");
    assert_eq!(preview.affected_files, ["src/lib.rs"]);

    let explanation = patch_provider
        .explain_patch(&workspace, &patch)
        .expect("patch explanation loads");
    assert_eq!(explanation.policy_categories, ["code.patch.apply"]);

    let rejected = patch_provider
        .reject_patch(&workspace, "patch.1", "needs tests")
        .expect("patch rejection records");
    assert_eq!(rejected.reason, "needs tests");

    let rollback = patch_provider
        .rollback_patch(&workspace, "rollback.patch.1")
        .expect("patch rollback reports");
    assert_eq!(rollback.restored_files, ["src/lib.rs"]);
}

#[test]
fn terminal_command_declares_policy_working_directory_timeout_and_env_policy() {
    let command = TerminalCommand::new("command.1", "cargo", vec!["test".to_string()], "workspace")
        .with_timeout_ms(60_000)
        .with_env_policy(HostEnvPolicy::AllowList(vec!["PATH".to_string()]))
        .with_policy_categories(vec!["code.terminal.run".to_string()]);

    assert_eq!(command.command, "cargo");
    assert_eq!(command.working_directory, "workspace");
    assert_eq!(command.timeout_ms, Some(60_000));
    assert!(command.requires_policy());
}

#[test]
fn verification_report_preserves_command_evidence_and_failures() {
    let plan = VerificationPlan::new("verification.1", "workspace.1").add_command(
        TerminalCommand::new("command.1", "cargo", vec!["test".to_string()], "workspace"),
    );
    let report = VerificationReport::new("report.1", plan.verification_id.clone())
        .add_command_result(CommandResult::exited("command.1", 101, "", "test failed"))
        .add_failure("tests::it_fails");

    assert_eq!(report.verification_id, "verification.1");
    assert_eq!(report.command_results.len(), 1);
    assert_eq!(report.failures, ["tests::it_fails"]);
    assert!(!report.is_success());
}

#[test]
fn review_finding_has_stable_severity_location_and_test_gap() {
    let finding = ReviewFinding::new(
        "finding.1",
        ReviewSeverity::High,
        "src/lib.rs",
        42,
        "unsafe shell command",
    )
    .with_remediation("Route through TerminalProvider policy")
    .with_missing_test("terminal command policy denial");

    assert_eq!(finding.severity, ReviewSeverity::High);
    assert_eq!(finding.file_path, "src/lib.rs");
    assert_eq!(finding.line, Some(42));
    assert_eq!(
        finding.missing_test.as_deref(),
        Some("terminal command policy denial")
    );
}

struct FakeWorkspaceProvider;

impl WorkspaceProvider for FakeWorkspaceProvider {
    fn list_files(
        &self,
        _workspace: &Workspace,
        root: &str,
    ) -> KernelResult<Vec<sdkwork_code_kernel::WorkspaceFileEntry>> {
        Ok(vec![
            sdkwork_code_kernel::WorkspaceFileEntry::file("src/lib.rs"),
            sdkwork_code_kernel::WorkspaceFileEntry::directory(root),
        ])
    }

    fn read_file(&self, _workspace: &Workspace, path: &str) -> KernelResult<WorkspaceFile> {
        if path.starts_with("../") {
            return Err(KernelError::PolicyDenied {
                reason_code: "code.workspace.path_denied".to_string(),
            });
        }

        Ok(WorkspaceFile::with_content(path, "pub fn main() {}"))
    }

    fn write_file(
        &self,
        _workspace: &Workspace,
        request: sdkwork_code_kernel::WorkspaceWriteRequest,
    ) -> KernelResult<sdkwork_code_kernel::WorkspaceWriteResult> {
        Ok(
            sdkwork_code_kernel::WorkspaceWriteResult::written(request.path, request.content.len())
                .with_version("abc124"),
        )
    }

    fn stat_file(
        &self,
        _workspace: &Workspace,
        path: &str,
    ) -> KernelResult<sdkwork_code_kernel::WorkspaceFileStat> {
        Ok(sdkwork_code_kernel::WorkspaceFileStat::file(path, 16).with_version("abc123"))
    }

    fn watch_events(
        &self,
        _workspace: &Workspace,
        _since_cursor: Option<&str>,
    ) -> KernelResult<Vec<sdkwork_code_kernel::WorkspaceWatchEvent>> {
        Ok(vec![sdkwork_code_kernel::WorkspaceWatchEvent::new(
            "event.workspace.1",
            "src/lib.rs",
            sdkwork_code_kernel::WorkspaceWatchEventKind::Modified,
            "cursor.1",
        )])
    }

    fn health(&self) -> ProviderHealth {
        ProviderHealth::available()
    }
}

struct FakeVcsProvider;

impl VcsProvider for FakeVcsProvider {
    fn snapshot(&self, _workspace: &Workspace) -> KernelResult<VcsSnapshot> {
        Ok(VcsSnapshot::new("main")
            .with_head_revision("abc123")
            .with_changed_files(vec!["src/lib.rs".to_string()])
            .mark_dirty())
    }

    fn diff(
        &self,
        _workspace: &Workspace,
        _request: sdkwork_code_kernel::VcsDiffRequest,
    ) -> KernelResult<sdkwork_code_kernel::VcsDiff> {
        Ok(sdkwork_code_kernel::VcsDiff::new("diff.1")
            .add_file(sdkwork_code_kernel::VcsDiffFile::modified(
                "src/lib.rs",
                1,
                1,
            ))
            .with_summary("1 file changed"))
    }

    fn blame(
        &self,
        _workspace: &Workspace,
        path: &str,
    ) -> KernelResult<Vec<sdkwork_code_kernel::VcsBlameLine>> {
        Ok(vec![sdkwork_code_kernel::VcsBlameLine::new(
            path,
            1,
            "abc123",
            "SDKWork",
            "initial commit",
        )])
    }

    fn commit_metadata(
        &self,
        _workspace: &Workspace,
        revision: &str,
    ) -> KernelResult<sdkwork_code_kernel::VcsCommitMetadata> {
        Ok(sdkwork_code_kernel::VcsCommitMetadata::new(
            revision,
            "SDKWork",
            "initial commit",
        ))
    }

    fn restore(
        &self,
        _workspace: &Workspace,
        request: sdkwork_code_kernel::VcsRestoreRequest,
    ) -> KernelResult<sdkwork_code_kernel::VcsRestoreReport> {
        Ok(
            sdkwork_code_kernel::VcsRestoreReport::restored(request.paths)
                .with_revision(request.revision.unwrap_or_else(|| "HEAD".to_string())),
        )
    }

    fn health(&self) -> ProviderHealth {
        ProviderHealth::available()
    }
}

struct FakePatchProvider;

impl PatchProvider for FakePatchProvider {
    fn validate_patch(&self, _workspace: &Workspace, patch: &PatchSet) -> KernelResult<()> {
        if patch.operations.is_empty() {
            return Err(KernelError::validation("patch requires operations"));
        }

        Ok(())
    }

    fn preview_patch(
        &self,
        _workspace: &Workspace,
        patch: &PatchSet,
    ) -> KernelResult<sdkwork_code_kernel::PatchPreview> {
        Ok(sdkwork_code_kernel::PatchPreview::from_patch(patch))
    }

    fn apply_patch(
        &self,
        _workspace: &Workspace,
        patch: PatchSet,
    ) -> KernelResult<sdkwork_code_kernel::PatchApplyResult> {
        Ok(sdkwork_code_kernel::PatchApplyResult::applied(
            patch.patch_id,
            "rollback.patch.1",
        ))
    }

    fn reject_patch(
        &self,
        _workspace: &Workspace,
        patch_id: &str,
        reason: &str,
    ) -> KernelResult<sdkwork_code_kernel::PatchRejection> {
        Ok(sdkwork_code_kernel::PatchRejection::rejected(
            patch_id, reason,
        ))
    }

    fn rollback_patch(
        &self,
        _workspace: &Workspace,
        rollback_token: &str,
    ) -> KernelResult<sdkwork_code_kernel::PatchRollbackResult> {
        Ok(sdkwork_code_kernel::PatchRollbackResult::rolled_back(
            rollback_token,
            vec!["src/lib.rs".to_string()],
        ))
    }

    fn explain_patch(
        &self,
        _workspace: &Workspace,
        patch: &PatchSet,
    ) -> KernelResult<sdkwork_code_kernel::PatchExplanation> {
        Ok(sdkwork_code_kernel::PatchExplanation::new(
            patch.patch_id.clone(),
            patch.summary.clone(),
        )
        .add_risk_note("requires verification")
        .with_policy_categories(vec!["code.patch.apply".to_string()]))
    }

    fn health(&self) -> ProviderHealth {
        ProviderHealth::available()
    }
}
