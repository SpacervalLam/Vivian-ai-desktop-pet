//! Prepare first; replace runtime contributions only after every local check passes.
use super::*;
use crate::providers::protocol_registry;
use crate::skills::Skill;
use crate::tools::types::Tool;
use std::sync::Arc;

pub(super) struct PreparedPlugin {
    key: String,
    manifest: PluginManifest,
    fingerprint: String,
    skills: Vec<Skill>,
    tools: Vec<CustomToolDef>,
    js: js_host::PreparedJs,
    protocols: Vec<Arc<crate::providers::spec::CompiledSpec>>,
    mcp: Vec<McpServerConfig>,
}

pub(super) fn prepare(key: &str, authorized: bool) -> Result<PreparedPlugin, String> {
    if !valid_plugin_dir_name(key) {
        return Err("插件目录名无效".into());
    }
    let pdir = plugins_dir().join(key);
    if !pdir.is_dir() || !is_safe_plugin_path(&plugins_dir(), &pdir) {
        return Err(format!("插件目录不存在或越界：{key}"));
    }
    let (_, manifest) = read_manifest(&pdir).map_err(|(_, error)| error)?;
    let fingerprint = manifest_fingerprint(&pdir, &manifest);
    if !authorized && trust_status_of(key, &fingerprint) != "trusted" {
        return Err(format!("插件 {key} 未信任或内容已变更，请重新信任"));
    }
    if !is_plugin_enabled(key) {
        return Err("插件已禁用，请先启用".into());
    }
    let mut missing = missing_requirements(&pdir, &manifest);
    missing.extend(dependencies::runtime_missing(&manifest));
    if !missing.is_empty() {
        return Err(missing.join("；"));
    }
    if contrib_snapshots()
        .lock()
        .unwrap()
        .iter()
        .any(|(other, snapshot)| other != key && snapshot.name == manifest.name)
    {
        return Err(format!(
            "插件命名空间 {} 已被其他已加载插件占用",
            manifest.name
        ));
    }
    let mut paths: Vec<_> = manifest
        .tools
        .iter()
        .flat_map(|pattern| glob_json_files(&pdir, pattern))
        .collect();
    paths.sort();
    paths.dedup();
    let mut tools = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("读取工具 {}: {error}", path.display()))?;
        let mut definition: CustomToolDef = serde_json::from_str(&text)
            .map_err(|error| format!("工具 {}: {error}", path.display()))?;
        if !crate::tools::custom_tools::is_valid_tool_name(&definition.name) {
            return Err(format!("工具名无效：{}", definition.name));
        }
        if let Some(fragment) =
            crate::tools::custom_tools::forbidden_fragment_in(&definition.script)
        {
            return Err(format!(
                "工具 {} 包含禁止的脚本片段：{fragment}",
                definition.name
            ));
        }
        definition.parameters =
            crate::tools::custom_tools::sanitize_parameters(&definition.parameters)
                .ok_or_else(|| format!("工具 {} 参数 schema 必须为 object", definition.name))?;
        tools.push(definition);
    }
    let scripts: Vec<_> = tools.iter().map(|tool| tool.script.clone()).collect();
    let audits = if !scripts.is_empty() && tokio::runtime::Handle::try_current().is_ok() {
        // Startup may be entered from an async host. The blocking auditor owns
        // a runtime, so it must run outside that host's runtime context.
        std::thread::scope(|scope| {
            scope
                .spawn(|| crate::tools::ps_audit::audit_scripts_blocking(&scripts))
                .join()
        })
        .map_err(|_| "插件工具审计线程异常".to_string())??
    } else {
        crate::tools::ps_audit::audit_scripts_blocking(&scripts)?
    };
    for (tool, audit) in tools.iter().zip(audits) {
        if let Some(reason) = audit.summary() {
            return Err(format!("插件工具 {} 脚本审计未通过：{reason}", tool.name));
        }
    }
    let protocols = protocol_registry::prepare_plugin(&pdir)?;
    let js = js_host::prepare_one(&pdir)?;
    let mut tool_names = std::collections::HashSet::new();
    for name in tools
        .iter()
        .map(|tool| tool.name.as_str())
        .chain(js.report.tools.iter().map(|(_, name, _, _)| name.as_str()))
    {
        if !tool_names.insert(name) {
            return Err(format!("插件重复声明工具：{name}"));
        }
    }
    let mut skills: Vec<_> = expanded_skills(&pdir, &manifest.name, &manifest.skills)
        .into_iter()
        .map(|(name, description, keywords, body)| {
            Skill::global(name, description, body).with_keywords(keywords)
        })
        .collect();
    skills.extend(js.report.skills.iter().map(|(_, name, description, body)| {
        Skill::global(name.clone(), description.clone(), body.clone())
    }));
    let mut mcp = manifest.mcp_servers.clone();
    mcp.extend(js.report.mcp.iter().map(|(_, config)| config.clone()));
    let mut ids = std::collections::HashSet::new();
    for config in &mcp {
        config.validate().map_err(|error| error.to_string())?;
        if !ids.insert(&config.id) {
            return Err(format!("插件重复声明 MCP id：{}", config.id));
        }
    }
    let (_, current) = read_manifest(&pdir).map_err(|(_, error)| error)?;
    if fingerprint != manifest_fingerprint(&pdir, &current) {
        return Err("插件在校验期间发生变化，请重新加载".into());
    }
    Ok(PreparedPlugin {
        key: key.into(),
        manifest,
        fingerprint,
        skills,
        tools,
        js,
        protocols,
        mcp,
    })
}

fn register(
    prepared: PreparedPlugin,
    skills: &SkillService,
    tools: Option<&ToolSystem>,
    mcp_ids: Vec<String>,
) -> PluginLoadReport {
    let mut report = PluginLoadReport {
        plugins: vec![prepared.manifest.name.clone()],
        mcp_servers: mcp_ids.clone(),
        ..Default::default()
    };
    let mut snapshot = PluginContribSnapshot {
        fingerprint: prepared.fingerprint.clone(),
        name: prepared.manifest.name.clone(),
        version: prepared.manifest.version,
        dependencies: prepared.manifest.requirements.plugins,
        js: !prepared.manifest.js.is_empty(),
        mcp_ids,
        tool_definitions: Arc::new(prepared.tools.clone()),
        ..Default::default()
    };
    for skill in prepared.skills {
        report.skills.push(skill.name.clone());
        snapshot.skills.push(skill.name.clone());
        skills.replace_or_register(skill);
    }
    if let Some(tools) = tools {
        let mut definitions: Vec<Arc<dyn Tool>> = prepared
            .tools
            .into_iter()
            .map(|definition| Arc::new(DynamicTool::new(definition)) as Arc<dyn Tool>)
            .collect();
        definitions.extend(prepared.js.report.tools.iter().map(
            |(plugin, name, description, schema)| {
                Arc::new(js_host::JsPluginTool::new(
                    plugin.clone(),
                    name.clone(),
                    description.clone(),
                    schema.clone(),
                )) as Arc<dyn Tool>
            },
        ));
        for tool in definitions {
            let name = tool.name().to_string();
            if tools.has_tool(&name) {
                push_diag_event(format!(
                    "插件 {} 工具 {name} 与已注册工具重名，跳过",
                    prepared.manifest.name
                ));
                continue;
            }
            tools
                .register_tool_with_owner(tool, Some(format!("plugin:{}", prepared.manifest.name)));
            report.tools.push(name.clone());
            snapshot.tools.push(name);
        }
    }
    report.protocols.extend(
        prepared
            .protocols
            .iter()
            .map(|spec| spec.spec.provider_type.clone()),
    );
    let js_report = js_host::commit_one(prepared.js);
    report.protocols.extend(js_report.protocols);
    protocol_registry::replace_plugin(&prepared.key, prepared.protocols);
    inactive_plugins().lock().unwrap().remove(&prepared.key);
    store_snapshot(&prepared.key, snapshot);
    report
}

pub(super) fn start(
    prepared: PreparedPlugin,
    skills: &SkillService,
    mcp: &McpManager,
) -> Result<PluginLoadReport, String> {
    let added = mcp
        .try_merge_plugin_servers(&prepared.mcp, &prepared.manifest.name)
        .map_err(|error| error.to_string())?;
    Ok(register(prepared, skills, None, added))
}

struct Backup {
    snapshot: Option<PluginContribSnapshot>,
    skills: Vec<Skill>,
    tools: Vec<Arc<dyn Tool>>,
    mcp: Vec<McpServerConfig>,
}

impl Backup {
    fn capture(key: &str, skills: &SkillService, tools: &ToolSystem, mcp: &McpManager) -> Self {
        let snapshot = read_snapshot(key);
        let saved_skills = snapshot
            .as_ref()
            .into_iter()
            .flat_map(|snapshot| snapshot.skills.iter())
            .filter_map(|name| skills.find(name))
            .collect();
        let saved_tools = snapshot
            .as_ref()
            .into_iter()
            .flat_map(|snapshot| {
                snapshot.tools.iter().filter_map(|name| {
                    tools.find_owned_tool(name, &format!("plugin:{}", snapshot.name))
                })
            })
            .collect();
        let saved_mcp = mcp
            .load_configs()
            .into_iter()
            .filter(|config| {
                snapshot.as_ref().is_some_and(|snapshot| {
                    config.source_plugin.as_deref() == Some(snapshot.name.as_str())
                })
            })
            .collect();
        Self {
            snapshot,
            skills: saved_skills,
            tools: saved_tools,
            mcp: saved_mcp,
        }
    }

    async fn restore(
        self,
        skills: &SkillService,
        tools: &ToolSystem,
        mcp: &McpManager,
    ) -> Vec<String> {
        for skill in self.skills {
            skills.replace_or_register(skill);
        }
        let owner = self
            .snapshot
            .as_ref()
            .map(|snapshot| format!("plugin:{}", snapshot.name));
        for tool in self.tools {
            if !tools.has_tool(tool.name()) {
                tools.register_tool_with_owner(tool, owner.clone());
            }
        }
        let mut errors = Vec::new();
        for config in self.mcp {
            if let Err(error) = mcp.add_server(config).await {
                errors.push(error.to_string());
            }
        }
        errors
    }
}

pub(super) async fn load_locked(
    skills: &SkillService,
    mcp: &McpManager,
    tools: &ToolSystem,
    key: &str,
    authorized: bool,
) -> Result<PluginLoadReport, String> {
    invalidate_fingerprint_cache();
    let owned_key = key.to_string();
    let prepared = tokio::task::spawn_blocking(move || prepare(&owned_key, authorized))
        .await
        .map_err(|error| error.to_string())??;
    let old_trust = load_trust_store().trusted.get(key).cloned();
    if authorized {
        record_trust(key, &prepared.fingerprint)?;
    }
    let backup = Backup::capture(key, skills, tools, mcp);
    let inactive = inactive_plugins().lock().unwrap().remove(key);
    let pdir = plugins_dir().join(key);
    unload_contributions(skills, mcp, tools, &pdir).await;
    let mut failures = Vec::new();
    let added = match mcp.try_merge_plugin_servers(&prepared.mcp, &prepared.manifest.name) {
        Ok(added) => added,
        Err(error) => {
            failures.push(format!("保存 MCP 配置失败：{error}"));
            Vec::new()
        }
    };
    for id in &added {
        if let Some(config) = mcp
            .load_configs()
            .into_iter()
            .find(|config| config.id == *id)
        {
            if let Err(error) = mcp.add_server(config).await {
                failures.push(format!("MCP {id}: {error}"));
            }
        }
    }
    if !failures.is_empty() {
        mcp.remove_plugin_servers(&prepared.manifest.name).await;
        unload_contributions(skills, mcp, tools, &pdir).await;
        let mut rollback_errors = Vec::new();
        if authorized {
            let mut store = load_trust_store();
            if let Some(entry) = old_trust {
                store.trusted.insert(key.into(), entry);
            } else {
                store.trusted.remove(key);
            }
            if let Err(error) = save_trust_store(&store) {
                rollback_errors.push(format!("恢复信任状态失败：{error}"));
            }
        }
        rollback_errors.extend(
            backup
                .restore(skills, tools, mcp)
                .await
                .into_iter()
                .map(|error| format!("恢复旧连接失败：{error}")),
        );
        if let Some(reason) = inactive {
            inactive_plugins()
                .lock()
                .unwrap()
                .insert(key.into(), reason);
        }
        invalidate_fingerprint_cache();
        let recovery = if rollback_errors.is_empty() {
            "已恢复旧运行时".to_string()
        } else {
            format!("旧运行时恢复不完整：{}", rollback_errors.join("；"))
        };
        let error = format!("插件 {key} 重载失败，{recovery}：{}", failures.join("；"));
        push_diag_event(error.clone());
        return Err(error);
    }
    let report = register(prepared, skills, Some(tools), added);
    invalidate_fingerprint_cache();
    set_last_report(&report);
    Ok(report)
}

pub(super) async fn stop_tree(
    skills: &SkillService,
    mcp: &McpManager,
    tools: &ToolSystem,
    key: &str,
) {
    for victim in dependencies::dependent_keys(key) {
        inactive_plugins().lock().unwrap().insert(
            victim.clone(),
            if victim == key {
                None
            } else {
                Some(key.into())
            },
        );
        unload_contributions(skills, mcp, tools, &plugins_dir().join(&victim)).await;
        js_host::unload_one(&victim);
        drop_snapshot(&victim);
    }
    invalidate_fingerprint_cache();
}

pub(super) async fn reconcile(skills: &SkillService, mcp: &McpManager, tools: &ToolSystem) {
    let mut active: Vec<_> = contrib_snapshots()
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    active.sort();
    for key in active {
        let Some(snapshot) = read_snapshot(&key) else {
            continue;
        };
        if !dependencies::runtime_missing_for(&snapshot.dependencies).is_empty() {
            let blocker = snapshot
                .dependencies
                .keys()
                .min()
                .cloned()
                .unwrap_or_else(|| key.clone());
            stop_tree(skills, mcp, tools, &key).await;
            inactive_plugins()
                .lock()
                .unwrap()
                .insert(key, Some(blocker));
        }
    }
    for pdir in ordered_plugin_dirs().0 {
        let key = pdir.file_name().unwrap().to_string_lossy().into_owned();
        let recover = inactive_plugins()
            .lock()
            .unwrap()
            .get(&key)
            .is_some_and(Option::is_some);
        if !recover || !is_plugin_enabled(&key) {
            continue;
        }
        let Ok((_, manifest)) = read_manifest(&pdir) else {
            continue;
        };
        if !missing_requirements(&pdir, &manifest).is_empty()
            || !dependencies::runtime_missing(&manifest).is_empty()
        {
            continue;
        }
        if let Err(error) = load_locked(skills, mcp, tools, &key, false).await {
            push_diag_event(format!("恢复依赖插件 {key} 失败：{error}"));
        }
    }
}
