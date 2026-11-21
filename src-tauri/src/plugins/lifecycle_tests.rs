use super::*;
use crate::providers::protocol_registry;
use serde_json::{json, Value};
use std::sync::Arc;

struct Plugin {
    key: String,
    dir: PathBuf,
}
impl Plugin {
    fn new(prefix: &str, mut manifest: Value) -> Self {
        assert!(
            std::env::var_os("VIVIAN_TEST_DATA_DIR").is_some(),
            "Use the isolated extension test runner"
        );
        let key = format!("{prefix}-{}", uuid::Uuid::new_v4().simple());
        let dir = plugins_dir().join(&key);
        std::fs::create_dir_all(&dir).unwrap();
        manifest["name"] = json!(key);
        manifest["version"] = json!("1.0.0");
        std::fs::write(dir.join("plugin.json"), manifest.to_string()).unwrap();
        Self { key, dir }
    }
    fn edit(&self, edit: impl FnOnce(&mut Value)) {
        let mut manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(self.dir.join("plugin.json")).unwrap())
                .unwrap();
        edit(&mut manifest);
        std::fs::write(self.dir.join("plugin.json"), manifest.to_string()).unwrap();
        invalidate_fingerprint_cache();
    }
    fn write(&self, relative: &str, contents: &str) {
        let path = self.dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
        invalidate_fingerprint_cache();
    }
    fn js(&self, contents: &str) {
        self.write("js/main.js", contents);
        self.edit(|m| m["js"] = json!(["js/*.js"]));
    }
    fn authorize(&self) {
        let (_, manifest) = read_manifest(&self.dir).unwrap();
        record_trust(&self.key, &manifest_fingerprint(&self.dir, &manifest)).unwrap();
    }
}
impl Drop for Plugin {
    fn drop(&mut self) {
        js_host::unload_one(&self.key);
        drop_snapshot(&self.key);
        inactive_plugins().lock().unwrap().remove(&self.key);
        let _ = std::fs::remove_dir_all(&self.dir);
        invalidate_fingerprint_cache();
    }
}
fn services() -> (Arc<SkillService>, Arc<ToolSystem>, McpManager) {
    let skills = SkillService::new();
    let tools = Arc::new(ToolSystem::new());
    let mcp = McpManager::new(tools.clone()).unwrap();
    (skills, tools, mcp)
}
fn protocol(id: &str, provider: &str) -> String {
    json!({"id":id,"provider_type":provider,"request":{"message_format":"chat_completions"}})
        .to_string()
}
fn tool_script(name: &str, output: &str) -> String {
    format!(
        "ctx.tool('{}','fixture',{{type:'object'}},function(){{return '{}';}});",
        name, output
    )
}

#[test]
fn extension_protocol_fingerprint_and_loader_cover_identical_files() {
    let plugin = Plugin::new("protocol-hash", json!({}));
    plugin.write("protocols/acme.json", &protocol("old", "hash-acme"));
    plugin.write(
        "protocols/nested/other.json",
        &protocol("nested", "hash-nested"),
    );
    for declaration in [
        None,
        Some("protocols"),
        Some("protocols/*.json"),
        Some("protocols/**/*.json"),
        Some("protocols/acme.json"),
        Some("protocols\\*.json"),
    ] {
        plugin.edit(|m| {
            m.as_object_mut().unwrap().remove("protocols");
            if let Some(value) = declaration {
                m["protocols"] = json!(value);
            }
        });
        let (_, manifest) = read_manifest(&plugin.dir).unwrap();
        let paths = protocol_paths(&plugin.dir, manifest.protocols.as_deref());
        assert!(!paths.is_empty(), "{declaration:?}");
        assert_eq!(paths, protocol_registry::plugin_protocol_paths(&plugin.dir));
        for path in paths {
            let before = manifest_fingerprint(&plugin.dir, &manifest);
            let contents = std::fs::read_to_string(&path).unwrap();
            std::fs::write(&path, format!("{contents} ")).unwrap();
            assert_ne!(
                before,
                manifest_fingerprint(&plugin.dir, &manifest),
                "{}",
                path.display()
            );
        }
    }
    plugin.edit(|m| m["protocols"] = json!("empty-a/*.json"));
    let (_, first) = read_manifest(&plugin.dir).unwrap();
    let first_hash = manifest_fingerprint(&plugin.dir, &first);
    plugin.edit(|m| m["protocols"] = json!("empty-b/*.json"));
    let (_, second) = read_manifest(&plugin.dir).unwrap();
    assert_ne!(first_hash, manifest_fingerprint(&plugin.dir, &second));
    assert!(protocol_paths(&plugin.dir, Some("../outside")).is_empty());
    assert_eq!(
        protocol_paths(&plugin.dir, Some("protocols/*.json")).len(),
        1
    );
    assert_eq!(protocol_paths(&plugin.dir, None).len(), 2);
}

#[test]
fn extension_plugin_fingerprints_are_stable_for_dependency_and_mcp_maps() {
    let plugin = Plugin::new(
        "stable-hash",
        json!({
            "requirements":{"plugins":{"a":"1","b":"2","c":"3"}},
            "mcp_servers":[{"id":"fixture","name":"fixture","command":"unused",
                "env":{"A":"a","B":"b","C":"c"},"headers":{"X-A":"a","X-B":"b","X-C":"c"}}]
        }),
    );
    let (_, manifest) = read_manifest(&plugin.dir).unwrap();
    let expected = manifest_fingerprint(&plugin.dir, &manifest);
    for _ in 0..32 {
        let (_, manifest) = read_manifest(&plugin.dir).unwrap();
        assert_eq!(manifest_fingerprint(&plugin.dir, &manifest), expected);
    }
    plugin.edit(|m| m["mcp_servers"][0]["env"]["B"] = json!("changed"));
    let (_, manifest) = read_manifest(&plugin.dir).unwrap();
    assert_ne!(manifest_fingerprint(&plugin.dir, &manifest), expected);
}

#[tokio::test]
async fn extension_file_protocols_follow_reload_unload_disable_and_delete() {
    let plugin = Plugin::new("protocol-life", json!({}));
    let (skills, tools, mcp) = services();
    let provider = format!("provider_{}", uuid::Uuid::new_v4().simple());
    plugin.write("protocols/acme.json", &protocol("old", &provider));
    load_one(&skills, &mcp, &tools, &plugin.key, true)
        .await
        .unwrap();
    assert_eq!(
        protocol_registry::spec_for(&provider).unwrap().spec.id,
        "old"
    );
    plugin.write("protocols/acme.json", &protocol("new", &provider));
    assert!(load_one(&skills, &mcp, &tools, &plugin.key, false)
        .await
        .is_err());
    assert_eq!(
        protocol_registry::spec_for(&provider).unwrap().spec.id,
        "old"
    );
    load_one(&skills, &mcp, &tools, &plugin.key, true)
        .await
        .unwrap();
    assert_eq!(
        protocol_registry::spec_for(&provider).unwrap().spec.id,
        "new"
    );
    unload_one(&skills, &mcp, &tools, &plugin.key)
        .await
        .unwrap();
    protocol_registry::invalidate();
    assert!(
        protocol_registry::spec_for(&provider).is_none(),
        "Unloading must not rediscover the files"
    );
    load_one(&skills, &mcp, &tools, &plugin.key, false)
        .await
        .unwrap();
    set_enabled(&skills, &mcp, &tools, &plugin.key, false)
        .await
        .unwrap();
    assert!(protocol_registry::spec_for(&provider).is_none());
    set_enabled(&skills, &mcp, &tools, &plugin.key, true)
        .await
        .unwrap();
    assert!(protocol_registry::spec_for(&provider).is_some());
    delete_plugin(&skills, &mcp, &tools, &plugin.key)
        .await
        .unwrap();
    assert!(protocol_registry::spec_for(&provider).is_none());
}

#[tokio::test]
async fn extension_startup_registers_the_approved_tool_snapshot_and_identity() {
    let plugin = Plugin::new("staged-tools", json!({"tools":["tools/*.json"]}));
    let (skills, tools, mcp) = services();
    let name = format!("t_{}", uuid::Uuid::new_v4().simple());
    let mut definition = json!({"name":name,"description":"approved","parameters":{"type":"object"},"script":"Write-Output approved","created_at":0});
    plugin.write("tools/tool.json", &definition.to_string());
    plugin.authorize();
    let report = load_all(&skills, &mcp);
    assert!(report.plugins.contains(&plugin.key));
    let (_, manifest) = read_manifest(&plugin.dir).unwrap();
    cached_fingerprint(&plugin.dir, &manifest);
    // Simulate an external edit within the cache TTL between startup phases.
    definition["description"] = json!("unapproved");
    definition["script"] = json!("Write-Output changed");
    std::fs::write(plugin.dir.join("tools/tool.json"), definition.to_string()).unwrap();
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(plugin.dir.join("plugin.json")).unwrap())
            .unwrap();
    manifest["name"] = json!("externally-renamed");
    std::fs::write(plugin.dir.join("plugin.json"), manifest.to_string()).unwrap();
    load_all_tools(&tools);
    assert_eq!(tools.find_tool(&name).unwrap().description(), "approved");
    assert!(tools
        .find_owned_tool(&name, &format!("plugin:{}", plugin.key))
        .is_some());
    set_enabled(&skills, &mcp, &tools, &plugin.key, false)
        .await
        .unwrap();
    assert!(!tools.has_tool(&name));
    delete_plugin(&skills, &mcp, &tools, &plugin.key)
        .await
        .unwrap();
}

#[tokio::test]
async fn extension_renamed_plugin_removes_old_ownership_and_preserves_user_tools() {
    let plugin = Plugin::new("rename", json!({}));
    let (skills, tools, mcp) = services();
    let first = format!("t_{}", uuid::Uuid::new_v4().simple());
    let replaced = format!("t_{}", uuid::Uuid::new_v4().simple());
    let next = format!("t_{}", uuid::Uuid::new_v4().simple());
    plugin.js(&format!(
        "{}{}ctx.skill('guide','guide','old');",
        tool_script(&first, "old"),
        tool_script(&replaced, "old")
    ));
    load_one(&skills, &mcp, &tools, &plugin.key, true)
        .await
        .unwrap();
    let user_tool: Arc<dyn crate::tools::types::Tool> = Arc::new(DynamicTool::new(CustomToolDef {
        name: replaced.clone(),
        description: "user".into(),
        parameters: json!({"type":"object"}),
        script: "Get-Date".into(),
        deferred: false,
        created_at: 0.0,
    }));
    tools.register_tool(user_tool.clone());
    let renamed = format!("renamed-{}", uuid::Uuid::new_v4().simple());
    plugin.edit(|m| m["name"] = json!(renamed));
    plugin.js(&tool_script(&next, "new"));
    load_one(&skills, &mcp, &tools, &plugin.key, true)
        .await
        .unwrap();
    assert!(!tools.has_tool(&first));
    assert!(tools.has_tool(&next));
    assert!(skills.find(&format!("{}/guide", plugin.key)).is_none());
    assert!(Arc::ptr_eq(
        &user_tool,
        &tools.find_tool(&replaced).unwrap()
    ));
    assert_eq!(
        js_host::call_plugin_tool(&renamed, &next, json!({})).unwrap(),
        "new"
    );
    unload_one(&skills, &mcp, &tools, &plugin.key)
        .await
        .unwrap();
    assert!(!tools.has_tool(&next));
    assert!(!is_plugin_name_trusted(&renamed));
    assert!(tools.has_tool(&replaced));
    tools.unregister_tool(&replaced);
}

#[tokio::test]
async fn extension_failed_reload_keeps_runtime_protocols_connections_and_trust() {
    let plugin = Plugin::new("rollback", json!({}));
    let (skills, tools, mcp) = services();
    let tool = format!("t_{}", uuid::Uuid::new_v4().simple());
    let provider = format!("provider_{}", uuid::Uuid::new_v4().simple());
    let old_server = format!("old-{}", &uuid::Uuid::new_v4().simple().to_string()[..20]);
    plugin.js(&format!(
        "{}ctx.skill('guide','guide','old');",
        tool_script(&tool, "old")
    ));
    plugin.write("protocols/acme.json", &protocol("old", &provider));
    plugin.edit(|m| {
        m["mcp_servers"] =
            json!([{"id":old_server,"name":"old","command":"unused-fixture","enabled":false}])
    });
    load_one(&skills, &mcp, &tools, &plugin.key, true)
        .await
        .unwrap();
    let fingerprint = load_trust_store().trusted[&plugin.key].fingerprint.clone();
    plugin.js("throw Error('invalid update');");
    assert!(load_one(&skills, &mcp, &tools, &plugin.key, true)
        .await
        .is_err());
    assert_eq!(
        js_host::call_plugin_tool(&plugin.key, &tool, json!({})).unwrap(),
        "old"
    );
    assert_eq!(
        load_trust_store().trusted[&plugin.key].fingerprint,
        fingerprint
    );
    plugin.js(&tool_script(&tool, "new"));
    plugin.write("protocols/acme.json", "{");
    assert!(load_one(&skills, &mcp, &tools, &plugin.key, true)
        .await
        .is_err());
    plugin.write("protocols/acme.json", &protocol("new", &provider));
    plugin.write("tools/broken.json", "{");
    plugin.edit(|m| m["tools"] = json!(["tools/*.json"]));
    assert!(load_one(&skills, &mcp, &tools, &plugin.key, true)
        .await
        .is_err());
    let failed_server = format!(
        "failed-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..20]
    );
    plugin.edit(|m| { m["name"] = json!("rollback-renamed"); m["tools"] = json!([]); m["mcp_servers"] = json!([{"id":failed_server,"name":"broken","command":"vivian-nonexistent-mcp-fixture-command"}]); });
    assert!(load_one(&skills, &mcp, &tools, &plugin.key, true)
        .await
        .is_err());
    assert!(tools.has_tool(&tool));
    assert_eq!(
        js_host::call_plugin_tool(&plugin.key, &tool, json!({})).unwrap(),
        "old"
    );
    assert!(skills.find(&format!("{}/guide", plugin.key)).is_some());
    assert_eq!(
        protocol_registry::spec_for(&provider).unwrap().spec.id,
        "old"
    );
    assert_eq!(
        load_trust_store().trusted[&plugin.key].fingerprint,
        fingerprint
    );
    assert!(mcp
        .load_configs()
        .iter()
        .any(|config| config.id == old_server));
    assert!(!mcp
        .load_configs()
        .iter()
        .any(|config| config.id == failed_server));
    delete_plugin(&skills, &mcp, &tools, &plugin.key)
        .await
        .unwrap();
}

#[tokio::test]
async fn extension_dependencies_load_in_order_stop_and_recover_transitively() {
    let base = Plugin::new("z-base", json!({}));
    let child = Plugin::new("b-child", json!({}));
    let leaf = Plugin::new("a-leaf", json!({}));
    let (skills, tools, mcp) = services();
    let names: Vec<_> = (0..3)
        .map(|_| format!("t_{}", uuid::Uuid::new_v4().simple()))
        .collect();
    base.js(&tool_script(&names[0], "base"));
    child.js(&tool_script(&names[1], "child"));
    leaf.js(&tool_script(&names[2], "leaf"));
    child.edit(|m| m["requirements"] = json!({"plugins":{base.key.clone():"1.0.0"}}));
    leaf.edit(|m| m["requirements"] = json!({"plugins":{child.key.clone():"1.0.0"}}));
    for plugin in [&base, &child, &leaf] {
        plugin.authorize();
    }
    let report = load_all(&skills, &mcp);
    load_all_tools(&tools);
    let position = |key: &String| report.plugins.iter().position(|name| name == key).unwrap();
    assert!(
        position(&base.key) < position(&child.key) && position(&child.key) < position(&leaf.key)
    );
    assert!(names.iter().all(|name| tools.has_tool(name)));
    set_enabled(&skills, &mcp, &tools, &base.key, false)
        .await
        .unwrap();
    assert!(names.iter().all(|name| !tools.has_tool(name)));
    assert_eq!(
        scan_inventory()
            .into_iter()
            .find(|plugin| plugin.key == leaf.key)
            .unwrap()
            .status,
        "blocked"
    );
    load_all(&skills, &mcp); // Simulate startup while the dependency stays disabled.
    set_enabled(&skills, &mcp, &tools, &base.key, true)
        .await
        .unwrap();
    assert!(names.iter().all(|name| tools.has_tool(name)));
    unload_one(&skills, &mcp, &tools, &child.key).await.unwrap();
    load_one(&skills, &mcp, &tools, &base.key, false)
        .await
        .unwrap();
    assert!(
        !tools.has_tool(&names[1]),
        "Manual unload must not be undone by unrelated reloads"
    );
    load_one(&skills, &mcp, &tools, &child.key, false)
        .await
        .unwrap();
    assert!(tools.has_tool(&names[2]));
    base.edit(|m| m["version"] = json!("0.5.0"));
    load_one(&skills, &mcp, &tools, &base.key, true)
        .await
        .unwrap();
    assert!(!tools.has_tool(&names[1]) && !tools.has_tool(&names[2]));
    base.edit(|m| m["version"] = json!("1.0.0"));
    load_one(&skills, &mcp, &tools, &base.key, true)
        .await
        .unwrap();
    assert!(names.iter().all(|name| tools.has_tool(name)));
    for plugin in [&leaf, &child, &base] {
        delete_plugin(&skills, &mcp, &tools, &plugin.key)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn extension_dependency_cycles_and_failed_dependencies_do_not_execute_dependents() {
    let first = Plugin::new("cycle-a", json!({}));
    let second = Plugin::new("cycle-b", json!({}));
    let (skills, tools, mcp) = services();
    first.edit(|m| m["requirements"] = json!({"plugins":{second.key.clone():"1"}}));
    second.edit(|m| m["requirements"] = json!({"plugins":{first.key.clone():"1"}}));
    first.authorize();
    second.authorize();
    let report = load_all(&skills, &mcp);
    assert!(report
        .skipped
        .iter()
        .any(|reason| reason.contains("循环依赖")));
    assert!(load_one(&skills, &mcp, &tools, &first.key, true)
        .await
        .is_err());
    assert!(read_snapshot(&first.key).is_none() && read_snapshot(&second.key).is_none());
    first.edit(|m| {
        m.as_object_mut().unwrap().remove("requirements");
    });
    first.js("throw Error('failed dependency');");
    first.authorize();
    second.js("throw Error('dependent must not execute');");
    second.authorize();
    let report = load_all(&skills, &mcp);
    assert!(report
        .skipped
        .iter()
        .any(|reason| reason.contains(&second.key) && reason.contains("尚未成功装载")));
    assert!(read_snapshot(&second.key).is_none());
    assert_eq!(parse_version("1.bad.3"), None);
    assert_eq!(parse_version("1.2.3.4"), None);
}
