//! Channel visibility only; Discord remains authoritative for every operation.
use serde_json::Value;
use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const VIEW_CHANNEL: u64 = 1 << 10;
const ADMINISTRATOR: u64 = 1 << 3;

fn bits(value: &Value) -> Result<u64, String> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .ok_or_else(|| "Invalid permission bitfield".to_string())
}

#[derive(Clone)]
pub struct ChannelPermissions {
    guild_id: String,
    user_id: String,
    roles: HashSet<String>,
    base: u64,
    bypass: bool,
}

impl ChannelPermissions {
    pub fn new(guild: &Value, member: &Value, user_id: &str) -> Result<Self, String> {
        let guild_id = guild["id"].as_str().ok_or("Missing guild id")?;
        let owner = guild["owner_id"].as_str().ok_or("Missing guild owner")?;
        let roles: HashSet<String> = member["roles"]
            .as_array()
            .ok_or("Missing member roles")?
            .iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or("Invalid member role"))
            .collect::<Result<_, _>>()?;
        let guild_roles = guild["roles"].as_array().ok_or("Missing guild roles")?;
        let mut found = HashSet::new();
        let mut base = 0;
        for role in guild_roles {
            let id = role["id"].as_str().ok_or("Missing role id")?;
            if id == guild_id || roles.contains(id) {
                base |= bits(&role["permissions"])?;
                found.insert(id.to_owned());
            }
        }
        if !found.contains(guild_id) || !roles.is_subset(&found) {
            return Err("Incomplete guild roles".into());
        }
        Ok(Self {
            guild_id: guild_id.into(),
            user_id: user_id.into(),
            roles,
            base,
            bypass: owner == user_id || base & ADMINISTRATOR != 0,
        })
    }

    pub fn can_view(&self, channel: &Value) -> Result<bool, String> {
        if self.bypass {
            return Ok(true);
        }
        let overwrites = channel["permission_overwrites"]
            .as_array()
            .ok_or("Missing channel overwrites")?;
        let mut everyone = (0, 0);
        let mut roles = (0, 0);
        let mut member = (0, 0);
        for overwrite in overwrites {
            let id = overwrite["id"].as_str().ok_or("Missing overwrite id")?;
            let kind = overwrite["type"].as_u64().ok_or("Invalid overwrite type")?;
            let target = match kind {
                0 if id == self.guild_id => &mut everyone,
                0 if self.roles.contains(id) => &mut roles,
                1 if id == self.user_id => &mut member,
                0 | 1 => continue,
                _ => return Err("Unknown overwrite type".into()),
            };
            target.0 |= bits(&overwrite["deny"])?;
            target.1 |= bits(&overwrite["allow"])?;
        }
        let mut permissions = self.base;
        for (deny, allow) in [everyone, roles, member] {
            permissions = (permissions & !deny) | allow;
        }
        Ok(permissions & VIEW_CHANNEL != 0)
    }
}

/// Servers whose permissions are kept, the last ones opened.
const CACHED_SERVERS: usize = 8;
/// A safety net: role and member changes already drop an entry as they arrive.
const CACHED_FOR: Duration = Duration::from_secs(300);

static CACHE: Mutex<VecDeque<(Instant, ChannelPermissions)>> = Mutex::new(VecDeque::new());

/// Permissions computed when the server was opened recently, so going back to it does not
/// fetch the whole server (roles, emojis, stickers) and the member again.
pub fn cached(guild_id: &str) -> Option<ChannelPermissions> {
    let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.iter()
        .find(|(at, p)| p.guild_id == guild_id && at.elapsed() < CACHED_FOR)
        .map(|(_, p)| p.clone())
}

pub fn remember(permissions: &ChannelPermissions) {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|(_, p)| p.guild_id != permissions.guild_id);
    cache.push_front((Instant::now(), permissions.clone()));
    cache.truncate(CACHED_SERVERS);
}

/// Roles, the server or the user's own membership changed.
pub fn forget(guild_id: &str) {
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).retain(|(_, p)| p.guild_id != guild_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn context(base: u64) -> ChannelPermissions {
        ChannelPermissions::new(
            &json!({"id":"g", "owner_id":"owner", "roles":[
                {"id":"g", "permissions":base.to_string()},
                {"id":"a", "permissions":"0"}, {"id":"b", "permissions":"0"}
            ]}),
            &json!({"roles":["a","b"]}),
            "me",
        )
        .unwrap()
    }
    fn overwrite(id: &str, kind: u64, deny: u64, allow: u64) -> Value {
        json!({"id":id,"type":kind,"deny":deny.to_string(),"allow":allow.to_string()})
    }
    fn channel(overwrites: Vec<Value>) -> Value {
        json!({"permission_overwrites":overwrites})
    }

    fn server(id: &str) -> ChannelPermissions {
        ChannelPermissions::new(&json!({"id":id, "owner_id":"o", "roles":[{"id":id, "permissions":"0"}]}), &json!({"roles":[]}), "me").unwrap()
    }

    #[test]
    fn cache_keeps_the_last_servers_and_forgets_on_request() {
        for i in 0..CACHED_SERVERS + 2 {
            remember(&server(&format!("cache-{i}")));
        }
        assert!(cached("cache-0").is_none() && cached("cache-1").is_none(), "the oldest go first");
        assert!(cached(&format!("cache-{}", CACHED_SERVERS + 1)).is_some());
        remember(&server("cache-2"));
        assert_eq!(CACHE.lock().unwrap().iter().filter(|(_, p)| p.guild_id == "cache-2").count(), 1, "no duplicates");
        forget("cache-2");
        assert!(cached("cache-2").is_none());
    }

    #[test]
    fn view_is_independent_of_connect_and_history() {
        assert!(context(VIEW_CHANNEL).can_view(&channel(vec![])).unwrap());
        assert!(!context((1 << 20) | (1 << 16))
            .can_view(&channel(vec![]))
            .unwrap());
    }
    #[test]
    fn everyone_deny_then_role_allow_then_member_deny() {
        let mut rules = vec![overwrite("g", 0, VIEW_CHANNEL, 0)];
        assert!(!context(VIEW_CHANNEL)
            .can_view(&channel(rules.clone()))
            .unwrap());
        rules.push(overwrite("a", 0, 0, VIEW_CHANNEL));
        assert!(context(VIEW_CHANNEL)
            .can_view(&channel(rules.clone()))
            .unwrap());
        rules.push(overwrite("me", 1, VIEW_CHANNEL, 0));
        assert!(!context(VIEW_CHANNEL).can_view(&channel(rules)).unwrap());
    }
    #[test]
    fn role_allow_wins_over_other_role_deny_regardless_of_order() {
        let mut rules = vec![
            overwrite("a", 0, 0, VIEW_CHANNEL),
            overwrite("b", 0, VIEW_CHANNEL, 0),
        ];
        assert!(context(0).can_view(&channel(rules.clone())).unwrap());
        rules.reverse();
        assert!(context(0).can_view(&channel(rules)).unwrap());
    }
    #[test]
    fn member_allow_wins_and_unrelated_members_do_not_apply() {
        assert!(context(0)
            .can_view(&channel(vec![overwrite("me", 1, 0, VIEW_CHANNEL)]))
            .unwrap());
        assert!(!context(0)
            .can_view(&channel(vec![overwrite("other", 1, 0, VIEW_CHANNEL)]))
            .unwrap());
    }
    #[test]
    fn owner_and_administrator_bypass_overwrites() {
        let ch = channel(vec![overwrite("me", 1, VIEW_CHANNEL, 0)]);
        assert!(context(ADMINISTRATOR).can_view(&ch).unwrap());
        let owner = ChannelPermissions::new(
            &json!({"id":"g","owner_id":"me","roles":[{"id":"g","permissions":"0"}]}),
            &json!({"roles":[]}),
            "me",
        )
        .unwrap();
        assert!(owner.can_view(&ch).unwrap());
    }
    #[test]
    fn incomplete_data_is_not_treated_as_permission() {
        assert!(context(0).can_view(&json!({})).is_err());
        assert!(context(0)
            .can_view(&channel(vec![
                json!({"id":"g","type":0,"deny":"bad","allow":"0"})
            ]))
            .is_err());
        assert!(ChannelPermissions::new(&json!({}), &json!({}), "me").is_err());
    }

    #[test]
    fn assigned_roles_grant_base_permissions_but_missing_roles_are_errors() {
        let guild = json!({"id":"g","owner_id":"owner","roles":[
            {"id":"g","permissions":"0"}, {"id":"a","permissions":"1024"}
        ]});
        let member = json!({"roles":["a"]});
        assert!(ChannelPermissions::new(&guild, &member, "me")
            .unwrap()
            .can_view(&channel(vec![]))
            .unwrap());
        assert!(ChannelPermissions::new(&guild, &json!({"roles":["missing"]}), "me").is_err());
    }
}
