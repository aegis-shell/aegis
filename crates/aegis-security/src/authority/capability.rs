/// One independently grantable Actor capability.
///
/// `ActorCapability` names what an Actor may attempt. Resource scope and live compositor
/// state are checked separately at authorization and commit time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum ActorCapability {
    ObserveWindows,
    ObserveWorkspaces,
    ObserveOutputs,
    ObserveNotifications,
    ObserveJournal,
    ObserveInteractionDomains,
    ObserveSettings,
    ObserveSystem,
    Focus,
    Minimize,
    Close,
    Move,
    SetWindowGeometry,
    InjectInput,
    InjectInteractionDomainInput,
    Cycle,
    SwitchWorkspace,
    SwitchWorkspaceTo,
    MoveToWorkspace,
    SystemControl,
    Notify,
    DismissNotification,
    Screenshot,
    ScreenshotRegion,
    ToggleOverview,
    CaptureOutput,
    StreamOutput,
    /// Connection-scoped global idle inhibition.
    IdleInhibit,
    /// Interactive, user-consent target picking.
    PickTarget,
    /// User-consent application picking.
    PickApp,
    /// User-consent secret prompting.
    PromptSecret,
    /// User-consent yes/no confirmation.
    PickConfirm,
    /// Desktop wallpaper mutation.
    SetWallpaper,
    /// Read one exact filesystem resource through a user- or policy-issued
    /// resource handle.
    ReadFile,
    /// Write one exact filesystem resource through a user- or policy-issued
    /// resource handle.
    WriteFile,
    /// Reach one exact network origin through a scoped network grant.
    AccessNetworkOrigin,
    /// Ask the human to approve a bounded payment request. This never grants
    /// direct payment-credential access.
    RequestPayment,
    CreateInteractionDomain,
    TransactInteractionDomain,
    RevokeInteractionDomain,
    CaptureInteractionDomain,
    /// Capture the real pixels of one authorized window, wherever it lives.
    CaptureWindow,
    /// Read semantic state without receiving framebuffer pixels.
    ObserveInteractionDomain,
    /// Publish validated accessibility trees as a dedicated semantic adapter.
    PublishAccessibilityTree,
    /// Receive and execute semantic actions through the accessibility API.
    DispatchAccessibilityAction,
    LaunchInInteractionDomain,
    /// Launch a desktop entry, optionally directing its first toplevel to a
    /// chosen or fresh workspace without switching the user's view (ADR-0118).
    LaunchApp,
}

impl ActorCapability {
    /// Parse one canonical capability spelling.
    pub fn from_name(name: &str) -> Option<Self> {
        let compact = name
            .trim()
            .chars()
            .filter(|character| *character != '_' && *character != '-')
            .flat_map(char::to_lowercase)
            .collect::<String>();
        Some(match compact.as_str() {
            "observewindows" => Self::ObserveWindows,
            "observeworkspaces" => Self::ObserveWorkspaces,
            "observeoutputs" => Self::ObserveOutputs,
            "observenotifications" => Self::ObserveNotifications,
            "observejournal" => Self::ObserveJournal,
            "observeinteractiondomains" => Self::ObserveInteractionDomains,
            "observesettings" => Self::ObserveSettings,
            "observesystem" => Self::ObserveSystem,
            "focus" => Self::Focus,
            "minimize" => Self::Minimize,
            "close" => Self::Close,
            "move" => Self::Move,
            "setwindowgeometry" => Self::SetWindowGeometry,
            "injectinput" => Self::InjectInput,
            "injectinteractiondomaininput" => Self::InjectInteractionDomainInput,
            "cycle" => Self::Cycle,
            "switchworkspace" => Self::SwitchWorkspace,
            "switchworkspaceto" => Self::SwitchWorkspaceTo,
            "movetoworkspace" => Self::MoveToWorkspace,
            "systemcontrol" => Self::SystemControl,
            "notify" => Self::Notify,
            "dismissnotification" => Self::DismissNotification,
            "screenshot" => Self::Screenshot,
            "screenshotregion" => Self::ScreenshotRegion,
            "toggleoverview" => Self::ToggleOverview,
            "captureoutput" => Self::CaptureOutput,
            "streamoutput" => Self::StreamOutput,
            "idleinhibit" => Self::IdleInhibit,
            "picktarget" => Self::PickTarget,
            "pickapp" => Self::PickApp,
            "promptsecret" => Self::PromptSecret,
            "pickconfirm" => Self::PickConfirm,
            "setwallpaper" => Self::SetWallpaper,
            "readfile" => Self::ReadFile,
            "writefile" => Self::WriteFile,
            "accessnetworkorigin" => Self::AccessNetworkOrigin,
            "requestpayment" => Self::RequestPayment,
            "createinteractiondomain" => Self::CreateInteractionDomain,
            "transactinteractiondomain" => Self::TransactInteractionDomain,
            "revokeinteractiondomain" => Self::RevokeInteractionDomain,
            "captureinteractiondomain" => Self::CaptureInteractionDomain,
            "capturewindow" => Self::CaptureWindow,
            "observeinteractiondomain" => Self::ObserveInteractionDomain,
            "publishaccessibilitytree" => Self::PublishAccessibilityTree,
            "dispatchaccessibilityaction" => Self::DispatchAccessibilityAction,
            "launchininteractiondomain" => Self::LaunchInInteractionDomain,
            "launchapp" => Self::LaunchApp,
            _ => return None,
        })
    }

    /// Short label for consent and permission-management surfaces.
    pub fn label(self) -> &'static str {
        match self {
            Self::ObserveWindows => "Observe window metadata",
            Self::ObserveWorkspaces => "Observe workspace state",
            Self::ObserveOutputs => "Observe display metadata",
            Self::ObserveNotifications => "Observe notifications",
            Self::ObserveJournal => "Observe desktop event history",
            Self::ObserveInteractionDomains => "Observe Actor interaction authority",
            Self::ObserveSettings => "Observe desktop settings",
            Self::ObserveSystem => "Observe live system status",
            Self::Focus => "Focus windows",
            Self::Minimize => "Minimize windows",
            Self::Close => "Close windows",
            Self::Move => "Move windows interactively",
            Self::SetWindowGeometry => "Resize and place windows",
            Self::InjectInput => "Inject synthetic input",
            Self::InjectInteractionDomainInput => "Act in its Interaction Domain",
            Self::Cycle => "Cycle window focus",
            Self::SwitchWorkspace => "Switch workspace",
            Self::SwitchWorkspaceTo => "Switch to a workspace",
            Self::MoveToWorkspace => "Move windows to workspaces",
            Self::SystemControl => "Control the session",
            Self::Notify => "Send notifications",
            Self::DismissNotification => "Dismiss notifications",
            Self::Screenshot => "Take screenshots",
            Self::ScreenshotRegion => "Take region screenshots",
            Self::ToggleOverview => "Toggle the overview",
            Self::CaptureOutput => "Capture screen outputs",
            Self::StreamOutput => "Stream screen outputs",
            Self::IdleInhibit => "Inhibit idle",
            Self::PickTarget => "Pick screen targets",
            Self::PickApp => "Pick applications",
            Self::PromptSecret => "Prompt for secrets",
            Self::PickConfirm => "Show confirmation dialogs",
            Self::SetWallpaper => "Set the wallpaper",
            Self::ReadFile => "Read an approved file",
            Self::WriteFile => "Write an approved file",
            Self::AccessNetworkOrigin => "Access an approved network origin",
            Self::RequestPayment => "Request a confirmed payment",
            Self::CreateInteractionDomain => "Create Agent Interaction Domains",
            Self::TransactInteractionDomain => "Transfer Interaction Domain authority",
            Self::RevokeInteractionDomain => "Revoke Agent Interaction Domains",
            Self::CaptureInteractionDomain => "Capture its Interaction Domain",
            Self::CaptureWindow => "Capture window contents",
            Self::ObserveInteractionDomain => "Observe its semantic objects",
            Self::PublishAccessibilityTree => "Publish application accessibility trees",
            Self::DispatchAccessibilityAction => "Dispatch application accessibility actions",
            Self::LaunchInInteractionDomain => "Launch apps in its Interaction Domain",
            Self::LaunchApp => "Launch applications",
        }
    }
}

/// Three-way result of evaluating a capability and resource scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationDecision {
    Permit,
    Ask(ActorCapability),
    Deny,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_canonical_capability_names() {
        assert_eq!(
            ActorCapability::from_name("inject_interaction_domain_input"),
            Some(ActorCapability::InjectInteractionDomainInput)
        );
        assert_eq!(
            ActorCapability::from_name("capture-window"),
            Some(ActorCapability::CaptureWindow)
        );
        assert_eq!(
            ActorCapability::from_name("launch_app"),
            Some(ActorCapability::LaunchApp)
        );
        assert_eq!(ActorCapability::from_name("InjectRealmInput"), None);
    }

    #[test]
    fn serde_rejects_legacy_and_writes_canonical_variants() {
        assert!(serde_json::from_str::<ActorCapability>(r#"{"type":"ObserveRealm"}"#).is_err());
        let capability = ActorCapability::ObserveInteractionDomain;
        assert_eq!(
            serde_json::to_string(&capability).unwrap(),
            r#"{"type":"ObserveInteractionDomain"}"#
        );
    }
}
