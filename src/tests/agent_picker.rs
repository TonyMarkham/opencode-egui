use crate::app::OpenCodeApp;
use crate::types::agent::AgentInfo;

fn sample_agents() -> Vec<AgentInfo> {
    vec![
        AgentInfo {
            name: "build".to_string(),
            description: None,
            mode: None,
            built_in: true,
            color: None,
        },
        AgentInfo {
            name: "plan.sub".to_string(),
            description: None,
            mode: Some("subagent".to_string()),
            built_in: false,
            color: None,
        },
    ]
}

#[test]
fn filtered_agents_hide_subagents_by_default() {
    let agents = sample_agents();
    let filtered = OpenCodeApp::filtered_agents(false, &agents);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].name, "build");
}

#[test]
fn filtered_agents_include_subagents_when_enabled() {
    let agents = sample_agents();
    let filtered = OpenCodeApp::filtered_agents(true, &agents);
    assert_eq!(filtered.len(), 2);
}

#[test]
fn ensure_tab_agent_keeps_valid_selection() {
    let agents = sample_agents();
    let filtered = OpenCodeApp::filtered_agents(false, &agents);
    let mut tab = OpenCodeApp::test_tab_with_agent(Some("build".to_string()));
    OpenCodeApp::ensure_tab_agent("build", &mut tab, &filtered);
    assert_eq!(tab.selected_agent.as_deref(), Some("build"));
}

#[test]
fn ensure_tab_agent_resets_to_default_when_missing() {
    let agents = sample_agents();
    let filtered = OpenCodeApp::filtered_agents(false, &agents);
    let mut tab = OpenCodeApp::test_tab_with_agent(Some("gone".to_string()));
    OpenCodeApp::ensure_tab_agent("build", &mut tab, &filtered);
    assert_eq!(tab.selected_agent.as_deref(), Some("build"));
}
