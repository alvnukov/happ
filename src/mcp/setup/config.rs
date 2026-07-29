//! The MCP server entry itself: the stanza that makes happ's tools exist.
//!
//! Three clients, three shapes. Every function here takes the file as it stands
//! and returns what it should become, so the caller decides whether that is
//! different enough to be worth a write.

use serde_json::{json, Map as JsonMap, Value as JsonValue};

use crate::mcp::Error;

pub(super) fn claude_entry(command: &str, args: &[String]) -> JsonValue {
    json!({ "command": command, "args": args })
}

pub(super) fn opencode_entry(command: &str, args: &[String]) -> JsonValue {
    let mut argv = vec![JsonValue::String(command.to_string())];
    argv.extend(args.iter().map(|arg| JsonValue::String(arg.clone())));
    json!({ "type": "local", "command": argv, "enabled": true })
}

/// Inserts `happ` under `section`, leaving every other key of the file intact.
pub(super) fn merge_json(existing: &str, section: &str, entry: JsonValue) -> Result<String, Error> {
    let mut root: JsonValue = if existing.trim().is_empty() {
        JsonValue::Object(JsonMap::new())
    } else {
        serde_json::from_str(existing)
            .map_err(|err| Error::Setup(format!("existing config is not valid JSON: {err}")))?
    };
    if !root.is_object() {
        return Err(Error::Setup(
            "existing config is not a JSON object".to_string(),
        ));
    }

    let servers = root
        .as_object_mut()
        .and_then(|map| {
            map.entry(section)
                .or_insert_with(|| JsonValue::Object(JsonMap::new()))
                .as_object_mut()
        })
        .ok_or_else(|| Error::Setup(format!("'{section}' in the existing config is not a map")))?;
    servers.insert("happ".to_string(), entry);

    encode_json(&root)
}

/// Drops `happ` from `section`, and `section` itself once it holds nothing
/// else, so uninstalling leaves the file as clean as it found it. `Ok(None)`
/// says happ was not there -- the caller writes nothing at all in that case,
/// which is what keeps a re-run from reformatting a config for no reason.
pub(super) fn without_json(existing: &str, section: &str) -> Result<Option<String>, Error> {
    if existing.trim().is_empty() {
        return Ok(None);
    }
    let mut root: JsonValue = serde_json::from_str(existing)
        .map_err(|err| Error::Setup(format!("existing config is not valid JSON: {err}")))?;
    let Some(root_map) = root.as_object_mut() else {
        return Err(Error::Setup(
            "existing config is not a JSON object".to_string(),
        ));
    };

    let Some(servers) = root_map.get_mut(section).and_then(JsonValue::as_object_mut) else {
        return Ok(None);
    };
    if servers.remove("happ").is_none() {
        return Ok(None);
    }
    if servers.is_empty() {
        root_map.remove(section);
    }
    if root_map.is_empty() {
        // A config file whose whole content would be `{}` says nothing that its
        // absence does not. Reporting it as empty lets the caller take the file
        // away instead of leaving a husk behind.
        return Ok(Some(String::new()));
    }

    encode_json(&root).map(Some)
}

fn encode_json(root: &JsonValue) -> Result<String, Error> {
    serde_json::to_string_pretty(root)
        .map(|text| format!("{text}\n"))
        .map_err(|err| Error::Setup(format!("encode config: {err}")))
}

pub(super) fn merge_codex_toml(
    existing: &str,
    command: &str,
    args: &[String],
) -> Result<String, Error> {
    let mut root: toml::Value = if existing.trim().is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        existing
            .parse()
            .map_err(|err| Error::Setup(format!("existing config is not valid TOML: {err}")))?
    };

    let table = root
        .as_table_mut()
        .ok_or_else(|| Error::Setup("existing config is not a TOML table".to_string()))?;
    let servers = table
        .entry("mcp_servers")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .ok_or_else(|| Error::Setup("'mcp_servers' is not a TOML table".to_string()))?;

    let mut entry = toml::map::Map::new();
    entry.insert(
        "command".to_string(),
        toml::Value::String(command.to_string()),
    );
    entry.insert(
        "args".to_string(),
        toml::Value::Array(
            args.iter()
                .map(|arg| toml::Value::String(arg.clone()))
                .collect(),
        ),
    );
    servers.insert("happ".to_string(), toml::Value::Table(entry));

    toml::to_string_pretty(&root).map_err(|err| Error::Setup(format!("encode config: {err}")))
}

/// The TOML counterpart of [`without_json`], with the same contract.
pub(super) fn without_codex_toml(existing: &str) -> Result<Option<String>, Error> {
    if existing.trim().is_empty() {
        return Ok(None);
    }
    let mut root: toml::Value = existing
        .parse()
        .map_err(|err| Error::Setup(format!("existing config is not valid TOML: {err}")))?;
    let Some(table) = root.as_table_mut() else {
        return Err(Error::Setup(
            "existing config is not a TOML table".to_string(),
        ));
    };

    let Some(servers) = table
        .get_mut("mcp_servers")
        .and_then(toml::Value::as_table_mut)
    else {
        return Ok(None);
    };
    if servers.remove("happ").is_none() {
        return Ok(None);
    }
    if servers.is_empty() {
        table.remove("mcp_servers");
    }

    toml::to_string_pretty(&root)
        .map(Some)
        .map_err(|err| Error::Setup(format!("encode config: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_claude_config_gets_the_stdio_command() {
        let written = merge_json(
            "",
            "mcpServers",
            claude_entry("/usr/bin/happ", &["mcp".into(), "--stdio".into()]),
        )
        .expect("merge");
        let parsed: JsonValue = serde_json::from_str(&written).expect("valid json");
        assert_eq!(parsed["mcpServers"]["happ"]["command"], "/usr/bin/happ");
        assert_eq!(parsed["mcpServers"]["happ"]["args"][1], "--stdio");
    }

    #[test]
    fn other_servers_in_the_config_survive() {
        let existing = r#"{"mcpServers":{"other":{"command":"other-server"}},"theme":"dark"}"#;
        let written = merge_json(existing, "mcpServers", claude_entry("happ", &[])).expect("merge");
        let parsed: JsonValue = serde_json::from_str(&written).expect("valid json");
        assert_eq!(parsed["mcpServers"]["other"]["command"], "other-server");
        assert_eq!(parsed["theme"], "dark", "unrelated keys must be preserved");
        assert!(parsed["mcpServers"]["happ"].is_object());
    }

    #[test]
    fn re_running_setup_replaces_only_the_happ_entry() {
        let first = merge_json("", "mcpServers", claude_entry("old", &[])).expect("merge");
        let second = merge_json(&first, "mcpServers", claude_entry("new", &[])).expect("merge");
        let parsed: JsonValue = serde_json::from_str(&second).expect("valid json");
        assert_eq!(parsed["mcpServers"]["happ"]["command"], "new");
        assert_eq!(
            parsed["mcpServers"].as_object().map(|map| map.len()),
            Some(1)
        );
    }

    #[test]
    fn merging_the_same_entry_twice_is_a_fixed_point() {
        // The caller compares against the file to decide whether to write, so
        // this equality is what makes a repeated setup a no-op.
        let once = merge_json("", "mcpServers", claude_entry("/usr/bin/happ", &[])).expect("merge");
        let twice =
            merge_json(&once, "mcpServers", claude_entry("/usr/bin/happ", &[])).expect("merge");
        assert_eq!(once, twice);

        let once = merge_codex_toml("model = \"o3\"\n", "/usr/bin/happ", &[]).expect("merge");
        let twice = merge_codex_toml(&once, "/usr/bin/happ", &[]).expect("merge");
        assert_eq!(once, twice);
    }

    #[test]
    fn a_broken_existing_config_is_refused_rather_than_overwritten() {
        assert!(merge_json("{not json", "mcpServers", claude_entry("happ", &[])).is_err());
        assert!(merge_codex_toml("not = = toml", "happ", &[]).is_err());
        assert!(without_json("{not json", "mcpServers").is_err());
        assert!(without_codex_toml("not = = toml").is_err());
    }

    #[test]
    fn removing_takes_out_happ_and_nothing_else() {
        let existing =
            r#"{"mcpServers":{"happ":{"command":"happ"},"other":{"command":"o"}},"theme":"dark"}"#;
        let written = without_json(existing, "mcpServers")
            .expect("remove")
            .expect("happ was registered");
        let parsed: JsonValue = serde_json::from_str(&written).expect("valid json");
        assert!(parsed["mcpServers"]["happ"].is_null());
        assert_eq!(parsed["mcpServers"]["other"]["command"], "o");
        assert_eq!(parsed["theme"], "dark", "unrelated keys must be preserved");
    }

    #[test]
    fn a_section_holding_only_happ_goes_away_with_it() {
        let written = without_json(r#"{"mcpServers":{"happ":{}},"theme":"dark"}"#, "mcpServers")
            .expect("remove")
            .expect("happ was registered");
        let parsed: JsonValue = serde_json::from_str(&written).expect("valid json");
        assert!(
            parsed.get("mcpServers").is_none(),
            "an empty server list is clutter, not configuration: {written}"
        );
        assert_eq!(parsed["theme"], "dark");
    }

    #[test]
    fn removing_what_was_never_registered_writes_nothing() {
        // Each of these is a way for happ to be absent, and none of them is a
        // reason to rewrite -- let alone reformat -- somebody else's config.
        assert!(without_json("", "mcpServers").expect("remove").is_none());
        assert!(without_json(r#"{"theme":"dark"}"#, "mcpServers")
            .expect("remove")
            .is_none());
        assert!(without_json(r#"{"mcpServers":{"other":{}}}"#, "mcpServers")
            .expect("remove")
            .is_none());
        assert!(without_codex_toml("").expect("remove").is_none());
        assert!(without_codex_toml("model = \"o3\"\n")
            .expect("remove")
            .is_none());
    }

    #[test]
    fn setup_then_remove_leaves_the_config_as_it_was() {
        let original = r#"{"theme":"dark","mcpServers":{"other":{"command":"o"}}}"#;
        let installed =
            merge_json(original, "mcpServers", claude_entry("/usr/bin/happ", &[])).expect("merge");
        let uninstalled = without_json(&installed, "mcpServers")
            .expect("remove")
            .expect("happ was registered");
        assert_eq!(
            uninstalled,
            encode_json(&serde_json::from_str::<JsonValue>(original).expect("valid json"))
                .expect("encode"),
            "a round trip must not leave anything behind"
        );

        let original = "model = \"o3\"\n";
        let installed = merge_codex_toml(original, "/usr/bin/happ", &[]).expect("merge");
        let uninstalled = without_codex_toml(&installed)
            .expect("remove")
            .expect("happ was registered");
        assert_eq!(uninstalled, original);
    }

    #[test]
    fn codex_gets_toml_under_mcp_servers() {
        let written = merge_codex_toml(
            "model = \"o3\"\n",
            "/usr/bin/happ",
            &["mcp".into(), "--stdio".into()],
        )
        .expect("merge");
        let parsed: toml::Value = written.parse().expect("valid toml");
        assert_eq!(parsed["model"].as_str(), Some("o3"));
        assert_eq!(
            parsed["mcp_servers"]["happ"]["command"].as_str(),
            Some("/usr/bin/happ")
        );
        assert_eq!(
            parsed["mcp_servers"]["happ"]["args"][1].as_str(),
            Some("--stdio")
        );
    }

    #[test]
    fn opencode_gets_command_as_one_argv_array() {
        let written = merge_json(
            "",
            "mcp",
            opencode_entry("/usr/bin/happ", &["mcp".into(), "--stdio".into()]),
        )
        .expect("merge");
        let parsed: JsonValue = serde_json::from_str(&written).expect("valid json");
        assert_eq!(parsed["mcp"]["happ"]["type"], "local");
        assert_eq!(parsed["mcp"]["happ"]["enabled"], true);
        assert_eq!(parsed["mcp"]["happ"]["command"][0], "/usr/bin/happ");
        assert_eq!(parsed["mcp"]["happ"]["command"][2], "--stdio");
    }
}
