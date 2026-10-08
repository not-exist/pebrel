//! CodeBuddy CLI hooks（`~/.codebuddy/settings.json` 的 `hooks` 对象）。
//!
//! CodeBuddy 的 hook 机制与 claude 同构：`EventName` → matcher 数组 →
//! `hooks` 数组，事件载荷经 stdin 传 JSON（snake_case，含 session_id /
//! hook_event_name）。差异有两点：hook 命令一律经 `$SHELL -c` 执行
//! （Windows 强制 Git Bash），且条目没有 `args` 字段——claude 用来规避
//! PowerShell 解析问题的 exec-form（#80）在这里不适用，所以三个平台统一
//! 写 shell 字符串，helper 路径沿用远端安装器的字面量引用。CodeBuddy
//! 自身会整文件重写 settings.json（2 空格缩进、键序不定），因此本模块
//! 只做纯 JSON 合并，不试图保留原文的格式排版。
//!
//! 与 claude/codex/kimi 同一套纪律：幂等合并、只认完整 helper 调用的
//! 条目、过期路径就地自愈、首次改动留 `*.pebrel-bak`、解析失败或形状
//! 意外拒写、目录不存在不 scaffold。

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::ai_hook::CODEBUDDY_EVENTS;
use crate::ai_hook::is_helper_shell_command;

/// hook 条目的 timeout（秒）。与 claude/kimi hook 的取值保持一致；
/// 条目一旦写入，用户手调过的值不会被自愈覆盖。
const CODEBUDDY_HOOK_TIMEOUT: i64 = 10;

/// CodeBuddy 的配置目录：`$CODEBUDDY_CONFIG_DIR`，否则 `~/.codebuddy`。
/// 已对照 2.150.0 安装 bundle 核实：主导解析模式为 trim 判空、非空原值
/// 作路径（个别调用点连 trim 都没有），settings.json 固定在其下；除本
/// 变量外没有其他 config 目录覆盖，`WORKBUDDY_CONFIG_DIR` 属于
/// `.workbuddy` 另一套 home，与本目录无关。trim 后为空按未设置处理，
/// 与 bundle 主导语义一致。
pub(super) fn codebuddy_config_dir() -> Option<PathBuf> {
    match std::env::var_os("CODEBUDDY_CONFIG_DIR") {
        Some(dir) if !dir.to_string_lossy().trim().is_empty() => {
            return Some(PathBuf::from(dir));
        },
        _ => {},
    }
    Some(crate::platform::dirs::home_dir()?.join(".codebuddy"))
}

/// 「自己的条目」= command 是指向 helper 的完整 shell 调用（以 `codebuddy`
/// 结尾）。本实现没有 args 字段，也不存在需要认领的历史 exec 形式，判定
/// 比 claude 版简化：只认 shell 字符串。
fn is_our_codebuddy_hook(entry: &Value) -> bool {
    if entry.get("type").is_some_and(|kind| kind != "command") {
        return false;
    }
    let Some(command) = entry.get("command").and_then(Value::as_str) else { return false };
    is_helper_shell_command(command, "codebuddy")
}

/// 纯 JSON 合并：每个订阅事件恰好一条 nebula-hook 命令，过期路径就地
/// 修好。`None` 表示文档形状与 CodeBuddy 文档不符，拒写。
fn install_into(root: &mut Value, command: &str) -> Option<bool> {
    // 三平台统一：CodeBuddy 的 hook 一律经 `$SHELL -c` 执行，写成单条
    // shell 字符串并沿用远端安装器的字面量引用；没有 args 字段，#80 的
    // exec 形式在这里不适用。
    let desired_command = format!("{} codebuddy", crate::ai_hook::remote::quote(command));
    let obj = root.as_object_mut()?;
    let hooks = obj.entry("hooks").or_insert_with(|| json!({})).as_object_mut()?;
    let mut changed = false;
    for event in CODEBUDDY_EVENTS {
        let matchers = hooks.entry(event).or_insert_with(|| json!([])).as_array_mut()?;
        let mut found = false;
        for matcher in matchers.iter_mut() {
            let Some(cmds) = matcher.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };
            for cmd in cmds {
                if !is_our_codebuddy_hook(cmd) {
                    continue;
                }
                found = true;
                let Some(entry) = cmd.as_object_mut() else { continue };
                // 只修路径：timeout 等字段可能被用户调过，不动（claude
                // 自愈同样只碰 command/args）。
                if entry.get("command").and_then(Value::as_str) != Some(&desired_command) {
                    entry.insert("command".into(), json!(desired_command));
                    changed = true;
                }
            }
        }
        if !found {
            matchers.push(json!({"hooks": [{
                "type": "command",
                "command": desired_command,
                "timeout": CODEBUDDY_HOOK_TIMEOUT,
            }]}));
            changed = true;
        }
    }
    Some(changed)
}

/// 任一订阅事件挂着我们的条目即视为已安装。残缺的旧配置同样可见，
/// 才能走 needs_repair 修复路径（与 claude 的 installed 语义一致）。
fn installed_in(root: &Value) -> bool {
    root.get("hooks").and_then(Value::as_object).is_some_and(|hooks| {
        CODEBUDDY_EVENTS.iter().any(|event| {
            hooks.get(*event).and_then(Value::as_array).is_some_and(|matchers| {
                matchers.iter().any(|matcher| {
                    matcher
                        .get("hooks")
                        .and_then(Value::as_array)
                        .is_some_and(|entries| entries.iter().any(is_our_codebuddy_hook))
                })
            })
        })
    })
}

/// 设置页只读复用同一 JSON 规则，区分部分安装和完整配置。
pub(super) fn inspect(raw: &str, helper: &str) -> std::io::Result<(bool, bool)> {
    let mut root: Value =
        serde_json::from_str(raw).map_err(|e| std::io::Error::other(e.to_string()))?;
    let installed = installed_in(&root);
    let current = install_into(&mut root, helper) == Some(false);
    Ok((installed, current))
}

/// 在 `path`（settings.json）上执行安装/自愈。返回是否写了文件。
fn ensure_codebuddy_hooks_in(path: &Path, command: &str) -> bool {
    let Ok(Some(_lock)) = crate::atomic_file::try_lock(path) else { return false };
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => Some(raw),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            log::warn!("ai_hook: cannot read {} ({error}); left alone", path.display());
            return false;
        },
    };
    let mut root: Value = match raw.as_deref() {
        // CodeBuddy 并发重写中途读到半截，或文件真的坏了：绝不“顺手修好”
        // 覆盖了事。watcher 会在下次变更时重试。
        Some(raw) => match serde_json::from_str(raw) {
            Ok(json) => json,
            Err(err) => {
                log::warn!("ai_hook: {} is not valid JSON ({err}); left alone", path.display());
                return false;
            },
        },
        None => json!({}),
    };
    let Some(changed) = install_into(&mut root, command) else {
        log::warn!("ai_hook: {} has an unexpected shape; left alone", path.display());
        return false;
    };
    // CodeBuddy 自己会整文件重写 settings.json，原文排版保不住：统一落
    // 2 空格缩进 + 结尾换行的规范形态。hook 未变且字节已一致时跳过写入
    // （幂等到字节）；CodeBuddy 重写过（键序/排版漂移）则重新归一。
    let Ok(mut canonical) = serde_json::to_string_pretty(&root) else { return false };
    canonical.push('\n');
    if !changed && raw.as_deref() == Some(canonical.as_str()) {
        return false;
    }
    // 首次改动留一份原始备份；新建文件没有可备份的原文，跳过。
    if path.exists() {
        let bak = path.with_extension("json.pebrel-bak");
        if !bak.exists() {
            if let Err(err) = std::fs::copy(path, &bak) {
                log::warn!("ai_hook: backup failed ({err}); not touching {}", path.display());
                return false;
            }
        }
    }
    match super::write_atomic(path, &canonical) {
        Ok(()) => {
            log::info!("ai_hook: codebuddy hooks installed into {}", path.display());
            true
        },
        Err(err) => {
            log::warn!("ai_hook: failed to write {}: {err}", path.display());
            false
        },
    }
}

/// 目录不存在 = 没有使用 CodeBuddy 的足迹，不为它 scaffold 配置树
/// （与 kimi/pi/opencode 一致）。
fn ensure_codebuddy_hooks_in_dir(dir: &Path, helper: &str) -> bool {
    if !dir.exists() {
        return false;
    }
    ensure_codebuddy_hooks_in(&dir.join("settings.json"), helper)
}

/// 自愈/安装入口。返回是否写了文件。
pub(super) fn ensure_codebuddy_hooks() -> bool {
    let Some(dir) = codebuddy_config_dir() else { return false };
    // 复用 claude 条目的 helper_command()：同一份「lossy 字符串 +
    // Windows 反斜杠统一成正斜杠」归一化。codebuddy 侧拿到的不是最终
    // 命令（install_into 还要经 remote::quote 引用成 shell 字符串，
    // Windows 上由 Git Bash 经 $SHELL -c 消费，同样要求正斜杠），但
    // 需要的中间值完全一致。
    let Some(helper) = super::helper_command() else { return false };
    let changed = ensure_codebuddy_hooks_in_dir(&dir, &helper);
    if changed {
        super::announce();
    }
    changed
}

/// 从文档中删掉我们的条目（及清空后留下的空 matcher）。返回是否有改动。
fn remove_codebuddy_hooks_from(root: &mut Value) -> bool {
    let mut changed = false;
    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        for event in CODEBUDDY_EVENTS {
            let Some(matchers) = hooks.get_mut(event).and_then(Value::as_array_mut) else {
                continue;
            };
            for matcher in matchers.iter_mut() {
                if let Some(cmds) = matcher.get_mut("hooks").and_then(Value::as_array_mut) {
                    let before = cmds.len();
                    cmds.retain(|entry| !is_our_codebuddy_hook(entry));
                    changed |= cmds.len() != before;
                }
            }
            let before = matchers.len();
            matchers
                .retain(|m| m.get("hooks").and_then(Value::as_array).is_none_or(|c| !c.is_empty()));
            changed |= matchers.len() != before;
        }
    }
    changed
}

/// 在 `path`（settings.json）上移除我们的条目。返回是否写了文件。
fn remove_codebuddy_hooks_in(path: &Path) -> std::io::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let _lock = crate::atomic_file::try_lock(path)?
        .ok_or_else(|| std::io::Error::other("CodeBuddy hook configuration is busy"))?;
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let mut root: Value =
        serde_json::from_str(&raw).map_err(|e| std::io::Error::other(e.to_string()))?;
    if !remove_codebuddy_hooks_from(&mut root) {
        return Ok(false);
    }
    // 与 ensure 同一规范形态：2 空格缩进 + 结尾换行。
    super::write_atomic(path, &(serde_json::to_string_pretty(&root)? + "\n"))?;
    Ok(true)
}

/// `setup-ai --remove` 的 codebuddy 分支。
pub(super) fn remove_codebuddy_hooks() -> std::io::Result<bool> {
    let Some(dir) = codebuddy_config_dir() else { return Ok(false) };
    remove_codebuddy_hooks_in(&dir.join("settings.json"))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{
        CODEBUDDY_EVENTS, CODEBUDDY_HOOK_TIMEOUT, ensure_codebuddy_hooks_in,
        ensure_codebuddy_hooks_in_dir, install_into, is_our_codebuddy_hook,
        remove_codebuddy_hooks_in,
    };

    const HELPER: &str = "C:/Program Files/Pebrel/runtime/pebrel-hook.exe";

    /// 三平台统一：`quote(HELPER) + " codebuddy"`，没有 args 字段。
    fn desired_command() -> String {
        format!("{} codebuddy", crate::ai_hook::remote::quote(HELPER))
    }

    #[test]
    fn installs_all_ten_events_with_the_documented_shape() {
        let mut root = json!({});
        assert_eq!(install_into(&mut root, HELPER), Some(true));
        for event in CODEBUDDY_EVENTS {
            assert_eq!(root["hooks"][event].as_array().map(Vec::len), Some(1));
            let entry = &root["hooks"][event][0]["hooks"][0];
            assert_eq!(entry["type"], json!("command"));
            assert_eq!(entry["command"], json!(desired_command()));
            assert_eq!(entry["timeout"].as_i64(), Some(CODEBUDDY_HOOK_TIMEOUT));
        }
        assert_eq!(install_into(&mut root, HELPER), Some(false), "二次安装必须幂等");
    }

    #[test]
    fn command_is_a_quoted_shell_string_without_args() {
        let mut root = json!({});
        assert_eq!(install_into(&mut root, HELPER), Some(true));
        for event in CODEBUDDY_EVENTS {
            let entry = &root["hooks"][event][0]["hooks"][0];
            assert!(entry.get("args").is_none(), "CodeBuddy 条目没有 args 字段");
            let command = entry["command"].as_str().expect("command is a string");
            assert!(command.starts_with('\''), "路径经字面量引用：{command}");
            assert!(command.ends_with(" codebuddy"), "子命令拼在 shell 字符串里：{command}");
        }
    }

    #[test]
    fn third_party_keys_survive_the_merge() {
        let mut root = json!({
            "enabledPlugins": {"example": true},
            "trustedDirectories": ["/tmp"],
        });
        assert_eq!(install_into(&mut root, HELPER), Some(true));
        assert_eq!(root["enabledPlugins"], json!({"example": true}));
        assert_eq!(root["trustedDirectories"], json!(["/tmp"]));
    }

    /// 对照 claude 的同款 12 轮断言：外来条目（echo、组合命令、指向 helper
    /// 但非完整调用的条目）在反复 install/remove 循环中字节不变，陈旧条目
    /// 原位修复、无重复追加。
    #[test]
    fn install_and_remove_preserve_commands_that_only_mention_the_helper() {
        let foreign = json!([
            {"type": "command", "command": "echo hello"},
            {"type": "command", "command": "echo pebrel-hook.exe"},
            {"type": "command", "command": "echo 'C:/pebrel-hook.exe' codebuddy"},
            {"type": "command", "command": "C:/pebrel-hook.exe.backup"},
            {"type": "command", "command": HELPER},
            {"type": "command", "command": format!("\"{HELPER}\" codebuddy && echo user")},
        ]);
        let stale = json!([{
            "type": "command",
            "command": "'D:/old/Nebula/runtime/nebula-hook.exe' codebuddy",
            "timeout": 30,
        }]);
        let mut root = json!({"hooks": {"Stop": [{"hooks": foreign}, {"hooks": stale}]}});
        // 第 0 轮：stale 条目原位修复，外来条目字节不变，无重复追加。
        assert_eq!(install_into(&mut root, HELPER), Some(true));
        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "不得为同一事件追加第二条 hook");
        assert_eq!(stop[0]["hooks"], foreign, "外来条目字节不变");
        assert_eq!(stop[1]["hooks"].as_array().map(Vec::len), Some(1));
        assert_eq!(stop[1]["hooks"][0]["command"], json!(desired_command()));
        assert_eq!(stop[1]["hooks"][0]["timeout"].as_i64(), Some(30), "用户字段不动");
        assert_eq!(install_into(&mut root, HELPER), Some(false));
        assert!(super::remove_codebuddy_hooks_from(&mut root));
        assert_eq!(root["hooks"]["Stop"], json!([{"hooks": foreign}]));
        assert!(!super::remove_codebuddy_hooks_from(&mut root));
        for _ in 0..12 {
            assert_eq!(install_into(&mut root, HELPER), Some(true));
            assert_eq!(root["hooks"]["Stop"][0]["hooks"], foreign);
            let installed = root.clone();
            assert_eq!(install_into(&mut root, HELPER), Some(false));
            assert_eq!(root, installed);
            assert!(super::remove_codebuddy_hooks_from(&mut root));
            assert_eq!(root["hooks"]["Stop"], json!([{"hooks": foreign}]));
            assert!(!super::remove_codebuddy_hooks_from(&mut root));
        }
    }

    #[test]
    fn stale_commands_are_healed_in_place_without_duplicates() {
        let mut root = json!({
            "hooks": {
                "Stop": [{
                    "hooks": [{
                        "type": "command",
                        "command": "'D:/old/Nebula/runtime/nebula-hook.exe' codebuddy",
                        "timeout": 30,
                    }]
                }]
            }
        });
        assert_eq!(install_into(&mut root, HELPER), Some(true));
        let stop = root["hooks"]["Stop"].as_array().expect("matchers");
        assert_eq!(stop.len(), 1, "不得为同一事件追加第二条 hook");
        let entry = &stop[0]["hooks"][0];
        assert_eq!(entry["command"], json!(desired_command()));
        assert_eq!(entry["timeout"], json!(30), "用户调过的字段不自愈");
        assert_eq!(install_into(&mut root, HELPER), Some(false));
    }

    #[test]
    fn removal_strips_our_entries_and_keeps_foreign_ones() {
        let ours = desired_command();
        let mut root = json!({
            "hooks": {
                "Stop": [
                    {"hooks": [{"type": "command", "command": "echo hello"}]},
                    {"matcher": "Bash", "hooks": [
                        {"type": "command", "command": ours},
                    ]},
                    {"hooks": [
                        {"type": "command", "command": "echo pebrel-hook.exe"},
                        {"type": "command", "command": format!("\"{HELPER}\" codebuddy")},
                    ]},
                ],
                "SessionEnd": [{"hooks": [
                    {"type": "command", "command": "notify-send done"},
                ]}],
            },
            "custom": true,
        });
        assert!(super::remove_codebuddy_hooks_from(&mut root));
        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "空 matcher 被清掉，外来 matcher 保留");
        assert_eq!(stop[0]["hooks"][0]["command"], json!("echo hello"));
        assert_eq!(stop[1]["hooks"][0]["command"], json!("echo pebrel-hook.exe"));
        assert_eq!(
            root["hooks"]["SessionEnd"][0]["hooks"][0]["command"],
            json!("notify-send done")
        );
        assert_eq!(root["custom"], json!(true), "第三方键不受影响");
        assert!(!super::remove_codebuddy_hooks_from(&mut root), "已删净，第二遍无改动");
    }

    #[test]
    fn removal_only_walks_subscribed_events() {
        let ours = desired_command();
        let mut root = json!({"hooks": {"Unrelated": [{"hooks": [
            {"type": "command", "command": ours},
        ]}]}});
        assert!(!super::remove_codebuddy_hooks_from(&mut root));
        assert_eq!(root["hooks"]["Unrelated"][0]["hooks"][0]["command"], json!(ours));
    }

    #[test]
    fn missing_file_is_created_without_a_backup() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        assert!(ensure_codebuddy_hooks_in(&path, HELPER));
        let raw = std::fs::read_to_string(&path).unwrap();
        let root: Value = serde_json::from_str(&raw).unwrap();
        for event in CODEBUDDY_EVENTS {
            assert_eq!(root["hooks"][event].as_array().map(Vec::len), Some(1));
        }
        // 新建文件没有可备份的原文，不落 .pebrel-bak。
        assert!(!path.with_extension("json.pebrel-bak").exists());
    }

    #[test]
    fn ensure_writes_are_idempotent_down_to_the_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        assert!(ensure_codebuddy_hooks_in(&path, HELPER));
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(!ensure_codebuddy_hooks_in(&path, HELPER));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
    }

    #[test]
    fn first_modification_backs_up_and_later_ones_keep_the_pristine_copy() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, r#"{"model": "example-model"}"#).unwrap();

        assert!(ensure_codebuddy_hooks_in(&path, HELPER));
        let bak = path.with_extension("json.pebrel-bak");
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            r#"{"model": "example-model"}"#,
            "备份是首次改动前的原文"
        );

        // 模拟 CodeBuddy 或用户再次改动后的第二次安装：备份必须保持原文。
        let mut root: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        root["user_key"] = json!(1);
        std::fs::write(&path, serde_json::to_string_pretty(&root).unwrap()).unwrap();
        assert!(ensure_codebuddy_hooks_in(&path, HELPER));
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            r#"{"model": "example-model"}"#,
            "二次修改不再覆盖备份"
        );
    }

    #[test]
    fn broken_json_is_left_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let broken = "{\"hooks\": ";
        std::fs::write(&path, broken).unwrap();

        assert!(!ensure_codebuddy_hooks_in(&path, HELPER));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
        assert!(!path.with_extension("json.pebrel-bak").exists());
    }

    #[test]
    fn unexpected_document_shape_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        // hooks 不是对象 → 与文档形状不符，拒写。
        let raw = r#"{"hooks": []}"#;
        std::fs::write(&path, raw).unwrap();
        assert!(!ensure_codebuddy_hooks_in(&path, HELPER));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }

    #[test]
    fn removal_restores_a_clean_document() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        assert!(ensure_codebuddy_hooks_in(&path, HELPER));
        assert!(remove_codebuddy_hooks_in(&path).unwrap());
        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for event in CODEBUDDY_EVENTS {
            let matchers = root["hooks"][event].as_array().expect("event key survives");
            assert!(matchers.is_empty(), "我们的条目删净，matcher 一并清空");
        }
        assert!(!remove_codebuddy_hooks_in(&path).unwrap(), "第二遍无改动");
    }

    /// 路径含单引号时 quote() 按 `'\\''` 转义，归属判定必须能解回来，
    /// 否则含 `'` 的安装路径装完即被判为外来条目、下次又追加一条。
    #[test]
    fn single_quoted_helper_paths_round_trip_through_the_ownership_check() {
        let helper = "C:/Users/it's me/Pebrel/pebrel-hook.exe";
        let mut root = json!({});
        assert_eq!(install_into(&mut root, helper), Some(true));
        let entry = &root["hooks"]["Stop"][0]["hooks"][0];
        assert_eq!(
            entry["command"],
            json!("'C:/Users/it'\\''s me/Pebrel/pebrel-hook.exe' codebuddy")
        );
        assert!(is_our_codebuddy_hook(entry));
        assert_eq!(install_into(&mut root, helper), Some(false), "判定为自有条目才幂等");
    }

    #[test]
    fn broken_json_makes_removal_fail_loudly() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let broken = "{\"hooks\": ";
        std::fs::write(&path, broken).unwrap();

        assert!(remove_codebuddy_hooks_in(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    }

    #[test]
    fn missing_directory_is_not_scaffolded() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("no-such-dir");
        assert!(!ensure_codebuddy_hooks_in_dir(&dir, HELPER));
        assert!(!dir.exists());
    }
}
