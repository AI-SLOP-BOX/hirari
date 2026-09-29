#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UserChange {
    pub timestamp: u64,
    pub user_id: u32,
    pub target_id: u32,
    pub new_value: f32,
    pub meta: String,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CloudCommit {
    pub hash: String,
    pub parent_hash: String,
    pub merge_parent_hash: String,
    pub timestamp: u64,
    pub user_id: u32,
    pub description: String,
    pub deltas: Vec<UserChange>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshots_are_deterministic_and_round_trip() {
        let mut state = CloudOrchestrator::new();
        state.add_remote_user(RemoteUser {
            id: 2,
            name: "B".into(),
            is_active: true,
        });
        state.add_remote_user(RemoteUser {
            id: 1,
            name: "A".into(),
            is_active: true,
        });
        state.set_permission(UserPermission {
            user_id: 1,
            permission: Permission::Editor,
        });
        state.push_change(UserChange {
            timestamp: 2,
            user_id: 1,
            target_id: 7,
            new_value: 0.4,
            meta: "x".into(),
        });
        let json = state.snapshot_json().unwrap();
        assert_eq!(
            CloudOrchestrator::from_snapshot_json(&json)
                .unwrap()
                .snapshot_json()
                .unwrap(),
            json
        );
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MergeConflict {
    pub target_id: u32,
    pub changes: Vec<UserChange>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RemoteUser {
    pub id: u32,
    pub name: String,
    pub is_active: bool,
}
impl RemoteUser {
    pub fn validate(&self) -> bool {
        self.id != 0
            && !self.name.trim().is_empty()
            && self.name.len() <= 256
            && !self.name.contains('\0')
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Permission {
    Viewer,
    Editor,
    Admin,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UserPermission {
    pub user_id: u32,
    pub permission: Permission,
}
impl UserPermission {
    pub fn can_edit(&self) -> bool {
        matches!(self.permission, Permission::Editor | Permission::Admin)
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CloudOrchestrator {
    pub is_syncing: bool,
    #[serde(skip)]
    pub server_url: String,
    #[serde(skip)]
    pub session_id: String,
    pub pending_changes: Vec<UserChange>,
    pub remote_users: Vec<RemoteUser>,
    pub permissions: Vec<UserPermission>,
    #[serde(default)]
    pub commits: std::collections::BTreeMap<String, CloudCommit>,
    #[serde(default)]
    pub head_commit_hash: String,
}

impl Default for CloudOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl CloudOrchestrator {
    pub fn begin_sync(&mut self) -> bool {
        if self.is_syncing {
            false
        } else {
            self.is_syncing = true;
            true
        }
    }
    pub fn end_sync(&mut self) {
        self.is_syncing = false;
    }
    pub fn connect(
        &mut self,
        server_url: impl Into<String>,
        session_id: impl Into<String>,
    ) -> bool {
        let server_url = server_url.into();
        let session_id = session_id.into();
        if server_url.trim().is_empty() || session_id.trim().is_empty() {
            return false;
        }
        self.server_url = server_url;
        self.session_id = session_id;
        self.is_syncing = true;
        true
    }
    pub fn disconnect(&mut self) {
        self.is_syncing = false;
        self.server_url.clear();
        self.session_id.clear();
    }
    pub fn server_url(&self) -> &str {
        &self.server_url
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn changes_for_target(&self, target_id: u32) -> Vec<UserChange> {
        if target_id == 0 {
            return Vec::new();
        }
        let mut out: Vec<_> = self
            .pending_changes
            .iter()
            .filter(|c| c.target_id == target_id)
            .cloned()
            .collect();
        out.sort_by_key(|c| (c.timestamp, c.user_id));
        out
    }
    pub fn snapshot_json(&self) -> Result<String, String> {
        if !self.audit_cloud() {
            return Err("invalid cloud state".into());
        }
        let mut snapshot = self.clone();
        snapshot
            .pending_changes
            .sort_by_key(|change| (change.target_id, change.timestamp, change.user_id));
        snapshot.remote_users.sort_by_key(|user| user.id);
        snapshot
            .permissions
            .sort_by_key(|permission| permission.user_id);
        serde_json::to_string(&snapshot).map_err(|e| e.to_string())
    }
    pub fn from_snapshot_json(json: &str) -> Result<Self, String> {
        let mut state: Self =
            serde_json::from_str(json).map_err(|e| format!("invalid cloud snapshot: {e}"))?;
        state.ensure_history_root();
        if !state.audit_cloud() {
            return Err("invalid cloud state".into());
        }
        Ok(state)
    }
    pub fn can_push_change(permission: Option<&UserPermission>) -> bool {
        permission.map(UserPermission::can_edit).unwrap_or(false)
    }
    pub fn add_remote_user(&mut self, user: RemoteUser) -> bool {
        if !user.validate()
            || self
                .remote_users
                .iter()
                .any(|u| u.id == user.id || u.name.trim().eq_ignore_ascii_case(user.name.trim()))
        {
            return false;
        }
        self.remote_users.push(user);
        true
    }
    pub fn upsert_remote_user(&mut self, user: RemoteUser) -> bool {
        if !user.validate()
            || self.remote_users.iter().any(|existing| {
                existing.id != user.id
                    && existing.name.trim().eq_ignore_ascii_case(user.name.trim())
            })
        {
            return false;
        }
        if let Some(existing) = self
            .remote_users
            .iter_mut()
            .find(|existing| existing.id == user.id)
        {
            *existing = user;
        } else {
            self.remote_users.push(user);
        }
        true
    }
    pub fn has_remote_user(&self, user_id: u32) -> bool {
        self.remote_users.iter().any(|user| user.id == user_id)
    }
    pub fn set_permission(&mut self, permission: UserPermission) -> bool {
        if permission.user_id == 0
            || !self
                .remote_users
                .iter()
                .any(|user| user.id == permission.user_id)
        {
            return false;
        }
        if let Some(current) = self
            .permissions
            .iter_mut()
            .find(|current| current.user_id == permission.user_id)
        {
            *current = permission;
        } else {
            self.permissions.push(permission);
        }
        true
    }
    pub fn remove_permission(&mut self, user_id: u32) -> bool {
        let before = self.permissions.len();
        self.permissions
            .retain(|permission| permission.user_id != user_id);
        before != self.permissions.len()
    }
    pub fn permission_for(&self, user_id: u32) -> Option<&UserPermission> {
        self.permissions.iter().find(|p| p.user_id == user_id)
    }
    pub fn can_user_edit(&self, user_id: u32) -> bool {
        self.permission_for(user_id)
            .is_some_and(UserPermission::can_edit)
    }
    pub fn remove_remote_user(&mut self, user_id: u32) -> bool {
        let before = self.remote_users.len();
        self.remote_users.retain(|u| u.id != user_id);
        let removed = before != self.remote_users.len();
        if removed {
            self.permissions
                .retain(|permission| permission.user_id != user_id);
            self.pending_changes
                .retain(|change| change.user_id != user_id);
        }
        removed
    }
    pub fn set_user_active(&mut self, user_id: u32, active: bool) -> bool {
        self.remote_users
            .iter_mut()
            .find(|u| u.id == user_id)
            .map(|u| {
                u.is_active = active;
                true
            })
            .unwrap_or(false)
    }
    pub fn pending_for_user(&self, user_id: u32) -> Vec<UserChange> {
        if user_id == 0 || !self.remote_users.iter().any(|u| u.id == user_id) {
            return Vec::new();
        }
        let mut changes: Vec<_> = self
            .pending_changes
            .iter()
            .filter(|change| change.user_id == user_id)
            .cloned()
            .collect();
        changes.sort_by_key(|change| (change.timestamp, change.target_id));
        changes
    }

    pub fn pending_change_count(&self) -> usize {
        self.pending_changes.len()
    }

    /// Drains the queued delta batch for delivery to the transport layer.
    pub fn pull_changes(&mut self) -> Vec<UserChange> {
        std::mem::take(&mut self.pending_changes)
    }
    pub fn active_users(&self) -> Vec<u32> {
        let mut ids: Vec<_> = self
            .remote_users
            .iter()
            .filter(|u| u.is_active)
            .map(|u| u.id)
            .collect();
        ids.sort_unstable();
        ids
    }
    pub fn new() -> Self {
        let mut state = Self {
            is_syncing: false,
            server_url: String::new(),
            session_id: String::new(),
            pending_changes: Vec::new(),
            remote_users: Vec::new(),
            permissions: Vec::new(),
            commits: std::collections::BTreeMap::new(),
            head_commit_hash: String::new(),
        };
        state.ensure_history_root();
        state
    }

    fn ensure_history_root(&mut self) {
        const ROOT_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
        self.commits
            .entry(ROOT_HASH.to_owned())
            .or_insert_with(|| CloudCommit {
                hash: ROOT_HASH.to_owned(),
                parent_hash: String::new(),
                merge_parent_hash: String::new(),
                timestamp: 0,
                user_id: 0,
                description: "Root Commit: Initial State".to_owned(),
                deltas: Vec::new(),
            });
        if self.head_commit_hash.is_empty() {
            self.head_commit_hash = ROOT_HASH.to_owned();
        }
    }

    pub fn head_commit_hash(&self) -> &str {
        &self.head_commit_hash
    }

    pub fn commit(&self, hash: &str) -> Option<&CloudCommit> {
        self.commits.get(hash)
    }

    pub fn copy_commit(&self, hash: &str) -> Option<CloudCommit> {
        self.commits.get(hash).cloned()
    }

    /// Inserts an already received immutable commit after checking its
    /// content hash and both parent references.
    pub fn import_commit(&mut self, commit: CloudCommit) -> bool {
        if commit.hash.is_empty()
            || self.commits.len() >= 1_000_000
            || commit.description.len() > 4096
            || !self.commits.contains_key(&commit.parent_hash)
            || (!commit.merge_parent_hash.is_empty()
                && !self.commits.contains_key(&commit.merge_parent_hash))
            || Self::commit_hash(
                &commit.parent_hash,
                &commit.merge_parent_hash,
                commit.timestamp,
                commit.user_id,
                &commit.description,
                &commit.deltas,
            ) != commit.hash
        {
            return false;
        }
        self.commits.insert(commit.hash.clone(), commit);
        true
    }

    pub fn commit_state(
        &mut self,
        user_id: u32,
        description: impl Into<String>,
        deltas: Vec<UserChange>,
    ) -> String {
        let description = description.into();
        if self.commits.len() >= 1_000_000
            || description.len() > 4096
            || deltas.len() > 1_000_000
            || deltas.iter().any(|change| {
                change.target_id == 0
                    || !change.new_value.is_finite()
                    || change.meta.len() > 1024
                    || change.meta.contains('\0')
            })
        {
            return self.head_commit_hash.clone();
        }
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos().min(u64::MAX as u128) as u64)
            .unwrap_or(0);
        let parent_hash = self.head_commit_hash.clone();
        let hash = Self::commit_hash(&parent_hash, "", timestamp, user_id, &description, &deltas);
        let commit = CloudCommit {
            hash: hash.clone(),
            parent_hash,
            merge_parent_hash: String::new(),
            timestamp,
            user_id,
            description,
            deltas,
        };
        self.commits.insert(hash.clone(), commit);
        self.head_commit_hash = hash.clone();
        hash
    }

    /// Merges two histories using the nearest shared ancestor, then resolves
    /// per-target conflicts by timestamp and user ID.
    pub fn merge_branch(&mut self, remote_head_hash: &str) -> bool {
        if !self.commits.contains_key(remote_head_hash) {
            return false;
        }
        let local_head_hash = self.head_commit_hash.clone();
        let local_ancestors = self.ancestor_distances(&local_head_hash);
        let remote_ancestors = self.ancestor_distances(remote_head_hash);
        let Some((lca_hash, _)) = local_ancestors
            .iter()
            .filter_map(|(hash, local_depth)| {
                remote_ancestors
                    .get(hash)
                    .map(|remote_depth| (hash.clone(), *local_depth + *remote_depth))
            })
            .min_by(|(left_hash, left_depth), (right_hash, right_depth)| {
                left_depth
                    .cmp(right_depth)
                    .then_with(|| left_hash.cmp(right_hash))
            })
        else {
            return false;
        };

        if lca_hash == remote_head_hash {
            return true;
        }
        if lca_hash == local_head_hash {
            self.head_commit_hash = remote_head_hash.to_owned();
            return true;
        }

        let local_changes = self.changes_since(&local_head_hash, &lca_hash);
        let remote_changes = self.changes_since(remote_head_hash, &lca_hash);
        let mut winners = std::collections::BTreeMap::<u32, UserChange>::new();
        for change in local_changes.into_iter().chain(remote_changes) {
            let replace = winners.get(&change.target_id).is_none_or(|previous| {
                (change.timestamp, change.user_id) > (previous.timestamp, previous.user_id)
            });
            if replace {
                winners.insert(change.target_id, change);
            }
        }
        let deltas = winners.into_values().collect::<Vec<_>>();
        let description = "Merge Branch: 3-way LWW-CRDT Conflict Resolution".to_owned();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos().min(u64::MAX as u128) as u64)
            .unwrap_or(0);
        let hash = Self::commit_hash(
            &local_head_hash,
            remote_head_hash,
            timestamp,
            9999,
            &description,
            &deltas,
        );
        self.commits.insert(
            hash.clone(),
            CloudCommit {
                hash: hash.clone(),
                parent_hash: local_head_hash,
                merge_parent_hash: remote_head_hash.to_owned(),
                timestamp,
                user_id: 9999,
                description,
                deltas,
            },
        );
        self.head_commit_hash = hash;
        true
    }

    fn ancestor_distances(&self, head: &str) -> std::collections::BTreeMap<String, usize> {
        let mut distances = std::collections::BTreeMap::new();
        let mut pending = vec![(head.to_owned(), 0usize)];
        while let Some((hash, depth)) = pending.pop() {
            if distances
                .get(&hash)
                .is_some_and(|previous| *previous <= depth)
            {
                continue;
            }
            distances.insert(hash.clone(), depth);
            if let Some(commit) = self.commits.get(&hash) {
                if !commit.parent_hash.is_empty() {
                    pending.push((commit.parent_hash.clone(), depth + 1));
                }
                if !commit.merge_parent_hash.is_empty() {
                    pending.push((commit.merge_parent_hash.clone(), depth + 1));
                }
            }
        }
        distances
    }

    fn changes_since(&self, head: &str, ancestor: &str) -> Vec<UserChange> {
        let mut pending = vec![head.to_owned()];
        let mut visited = std::collections::BTreeSet::new();
        let mut latest = std::collections::BTreeMap::<u32, UserChange>::new();
        while let Some(hash) = pending.pop() {
            if hash == ancestor || !visited.insert(hash.clone()) {
                continue;
            }
            let Some(commit) = self.commits.get(&hash) else {
                continue;
            };
            for change in &commit.deltas {
                let replace = latest.get(&change.target_id).is_none_or(|previous| {
                    (change.timestamp, change.user_id) > (previous.timestamp, previous.user_id)
                });
                if replace {
                    latest.insert(change.target_id, change.clone());
                }
            }
            pending.push(commit.parent_hash.clone());
            pending.push(commit.merge_parent_hash.clone());
        }
        latest.into_values().collect()
    }

    fn commit_hash(
        parent_hash: &str,
        merge_parent_hash: &str,
        timestamp: u64,
        user_id: u32,
        description: &str,
        deltas: &[UserChange],
    ) -> String {
        use sha2::{Digest, Sha256};
        let mut canonical = deltas.iter().collect::<Vec<_>>();
        canonical.sort_by_key(|change| (change.target_id, change.timestamp, change.user_id));
        let mut digest = Sha256::new();
        for text in [parent_hash, merge_parent_hash, description] {
            digest.update((text.len() as u64).to_le_bytes());
            digest.update(text.as_bytes());
        }
        digest.update(timestamp.to_le_bytes());
        digest.update(user_id.to_le_bytes());
        digest.update((canonical.len() as u64).to_le_bytes());
        for change in canonical {
            digest.update(change.target_id.to_le_bytes());
            digest.update(change.timestamp.to_le_bytes());
            digest.update(change.user_id.to_le_bytes());
            digest.update(change.new_value.to_bits().to_le_bytes());
            digest.update((change.meta.len() as u64).to_le_bytes());
            digest.update(change.meta.as_bytes());
        }
        format!("{:x}", digest.finalize())
    }

    /// INDUSTRIAL: Pushes project changes with absolute precision and cloud sovereignty.
    pub fn push_change(&mut self, change: UserChange) {
        // INDUSTRIAL: Implementation of high-performance change management.
        // Rust's safe memory management handles large sync streams with
        // absolute bit-accuracy and zero-latency.
        // Rust's SyncEngine ensures bit-accurate state distribution.
        if change.user_id == 0
            || change.target_id == 0
            || !change.new_value.is_finite()
            || change.meta.trim().is_empty()
            || change.meta.len() > 1024
            || change.meta.contains('\0')
        {
            return;
        }
        if self.pending_changes.iter().any(|pending| {
            pending.target_id == change.target_id
                && pending.user_id == change.user_id
                && pending.timestamp > change.timestamp
        }) {
            return;
        }
        self.pending_changes.retain(|pending| {
            !(pending.target_id == change.target_id
                && pending.user_id == change.user_id
                && pending.timestamp <= change.timestamp)
        });
        if self.pending_changes.len() < 1_000_000 {
            self.pending_changes.push(change);
        }
    }

    /// INDUSTRIAL: Resolves concurrent edits with industrial precision and creative sovereignty.
    pub fn resolve_conflicts(&mut self) {
        let mut winners = std::collections::BTreeMap::<u32, UserChange>::new();
        for change in self.pending_changes.drain(..) {
            match winners.get(&change.target_id) {
                Some(previous)
                    if (previous.timestamp, previous.user_id)
                        >= (change.timestamp, change.user_id) => {}
                _ => {
                    winners.insert(change.target_id, change);
                }
            }
        }
        self.pending_changes = winners.into_values().collect();
    }

    pub fn preview_conflicts(&self) -> Vec<MergeConflict> {
        let mut grouped: std::collections::BTreeMap<u32, Vec<UserChange>> =
            std::collections::BTreeMap::new();
        for change in &self.pending_changes {
            grouped
                .entry(change.target_id)
                .or_default()
                .push(change.clone());
        }
        for changes in grouped.values_mut() {
            changes.sort_by_key(|change| (change.timestamp, change.user_id));
        }
        grouped
            .into_iter()
            .filter_map(|(target_id, changes)| {
                (changes.len() > 1).then_some(MergeConflict { target_id, changes })
            })
            .collect()
    }
    pub fn merge_offline(
        &mut self,
        changes: Vec<UserChange>,
        permission: Option<&UserPermission>,
    ) -> Result<usize, &'static str> {
        if !Self::can_push_change(permission)
            || permission.is_some_and(|p| {
                !self
                    .remote_users
                    .iter()
                    .any(|u| u.id == p.user_id && u.is_active)
            })
        {
            return Err("permission denied");
        }
        let mut accepted = 0usize;
        for change in changes {
            let is_new = change.user_id != 0
                && change.target_id != 0
                && change.new_value.is_finite()
                && !change.meta.trim().is_empty()
                && change.meta.len() <= 1024
                && !change.meta.contains('\0')
                && !self.pending_changes.iter().any(|existing| {
                    existing.timestamp == change.timestamp
                        && existing.user_id == change.user_id
                        && existing.target_id == change.target_id
                });
            self.push_change(change);
            if is_new {
                accepted += 1;
            }
        }
        self.resolve_conflicts();
        Ok(accepted)
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide cloud synchronization graph.
    pub fn audit_cloud(&self) -> bool {
        self.pending_changes.iter().all(|change| {
            change.user_id != 0
                && change.target_id != 0
                && change.new_value.is_finite()
                && self
                    .remote_users
                    .iter()
                    .any(|user| user.id == change.user_id)
        }) && self.pending_changes.len() <= 1_000_000
            && self.pending_changes.iter().all(|change| {
                !change.meta.trim().is_empty()
                    && change.meta.len() <= 1024
                    && !change.meta.contains('\0')
            })
            && self.pending_changes.iter().enumerate().all(|(i, change)| {
                self.pending_changes[..i].iter().all(|previous| {
                    (previous.timestamp, previous.user_id, previous.target_id)
                        != (change.timestamp, change.user_id, change.target_id)
                })
            })
            && self.remote_users.len() <= 65_536
            && self.remote_users.iter().all(RemoteUser::validate)
            && self.remote_users.iter().enumerate().all(|(i, user)| {
                self.remote_users[..i]
                    .iter()
                    .all(|previous| previous.id != user.id)
            })
            && self.remote_users.iter().enumerate().all(|(i, user)| {
                self.remote_users[..i]
                    .iter()
                    .all(|previous| !previous.name.trim().eq_ignore_ascii_case(user.name.trim()))
            })
            && self.permissions.iter().enumerate().all(|(i, permission)| {
                permission.user_id != 0
                    && self
                        .remote_users
                        .iter()
                        .any(|user| user.id == permission.user_id)
                    && self.permissions[..i]
                        .iter()
                        .all(|previous| previous.user_id != permission.user_id)
            })
            && self.commits.len() <= 1_000_000
            && !self.head_commit_hash.is_empty()
            && self.commits.contains_key(&self.head_commit_hash)
            && self.commits.iter().all(|(hash, commit)| {
                if hash != &commit.hash
                    || commit.description.len() > 4096
                    || (!commit.parent_hash.is_empty()
                        && !self.commits.contains_key(&commit.parent_hash))
                    || (!commit.merge_parent_hash.is_empty()
                        && !self.commits.contains_key(&commit.merge_parent_hash))
                    || commit.deltas.len() > 1_000_000
                    || commit.deltas.iter().any(|change| {
                        change.target_id == 0
                            || !change.new_value.is_finite()
                            || change.meta.len() > 1024
                            || change.meta.contains('\0')
                    })
                {
                    return false;
                }
                let is_root = hash
                    == "0000000000000000000000000000000000000000000000000000000000000000"
                    && commit.parent_hash.is_empty()
                    && commit.merge_parent_hash.is_empty()
                    && commit.timestamp == 0
                    && commit.user_id == 0
                    && commit.description == "Root Commit: Initial State"
                    && commit.deltas.is_empty();
                is_root
                    || Self::commit_hash(
                        &commit.parent_hash,
                        &commit.merge_parent_hash,
                        commit.timestamp,
                        commit.user_id,
                        &commit.description,
                        &commit.deltas,
                    ) == *hash
            })
    }
}
