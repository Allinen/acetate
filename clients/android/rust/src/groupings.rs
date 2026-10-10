//! Groupings (collectives, eras, projects…) to browse and open, in listen mode: the list,
//! a grouping's releases, tracks, extras, and (from the server) its description and members.

use acetate_core::details::GroupingDetail;
use uuid::Uuid;

use crate::{album_row, parse_id, track_row, AlbumRow, Core, CoreError, ExtraRow, TrackRow};

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct GroupingRow {
    pub id: String,
    /// "collective", "era", "project"…
    pub kind: String,
    pub name: String,
    pub tracks: i64,
    pub albums: i64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct GroupingMemberRow {
    pub artist: String,
    pub name: String,
    /// On how many of its tracks they are credited (known once the server has answered).
    pub tracks: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct GroupingInfo {
    pub description: String,
    pub members: Vec<GroupingMemberRow>,
}

fn row(g: &acetate_core::catalog::model::Grouping) -> GroupingRow {
    GroupingRow { id: g.id.to_string(), kind: g.kind.clone(), name: g.name.clone(), tracks: g.track_count, albums: g.album_count }
}

#[uniffi::export]
impl Core {
    /// Every grouping whose name has all the words, by name.
    pub fn groupings(&self, filter: String) -> Vec<GroupingRow> {
        let Some(catalog) = self.shared.catalog() else { return vec![] };
        let words: Vec<String> = filter.to_lowercase().split_whitespace().map(str::to_string).collect();
        let mut list: Vec<GroupingRow> = catalog
            .groupings
            .iter()
            .filter(|g| {
                let name = g.name.to_lowercase();
                words.iter().all(|w| name.contains(w.as_str()))
            })
            .map(row)
            .collect();
        list.sort_by_key(|g| g.name.to_lowercase());
        list
    }

    pub fn grouping(&self, id: String) -> Option<GroupingRow> {
        let catalog = self.shared.catalog()?;
        catalog.grouping(parse_id(&id)?).map(row)
    }

    /// Its members as the catalog knows them (without the counts: `grouping_info` has those).
    pub fn grouping_members(&self, id: String) -> Vec<GroupingMemberRow> {
        let (Some(catalog), Some(id)) = (self.shared.catalog(), parse_id(&id)) else { return vec![] };
        let Some(g) = catalog.grouping(id) else { return vec![] };
        g.artists
            .iter()
            .filter_map(|a| catalog.artist(*a))
            .map(|a| GroupingMemberRow { artist: a.id.to_string(), name: a.label(), tracks: None })
            .collect()
    }

    /// Its releases, newest first.
    pub fn grouping_albums(&self, id: String) -> Vec<AlbumRow> {
        let (Some(catalog), Some(id)) = (self.shared.catalog(), parse_id(&id)) else { return vec![] };
        catalog.grouping_albums(id).iter().map(|&i| album_row(catalog.album_at(i))).collect()
    }

    /// Its tracks, newest release first.
    pub fn grouping_tracks(&self, id: String) -> Vec<TrackRow> {
        let (Some(catalog), Some(id)) = (self.shared.catalog(), parse_id(&id)) else { return vec![] };
        catalog.grouping_tracks(id).iter().map(|&i| track_row(&catalog, catalog.track_at(i))).collect()
    }

    /// The extras linked to it.
    pub fn grouping_extras(&self, id: String) -> Vec<ExtraRow> {
        let (Some(id), Some(catalog)) = (parse_id(&id), self.shared.catalog()) else { return vec![] };
        catalog
            .extras
            .iter()
            .filter(|e| e.links.iter().any(|(k, i)| k == "grouping" && *i == id))
            .map(crate::extra_row)
            .collect()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Core {
    /// What only the server tells: its description, and its members with their counts.
    pub async fn grouping_info(&self, id: String) -> Result<GroupingInfo, CoreError> {
        let id: Uuid = parse_id(&id).ok_or_else(|| failed("invalid", "That isn't an id."))?;
        let api = self.shared.api.clone();
        self.runtime
            .spawn(async move {
                let d: GroupingDetail = acetate_core::changes::get(&api, &format!("/api/groupings/{id}")).await.map_err(|e| failed("server", &e))?;
                Ok(GroupingInfo {
                    description: d.description,
                    members: d.members.into_iter().map(|m| GroupingMemberRow { artist: m.artist_id.to_string(), name: m.name, tracks: Some(m.tracks) }).collect(),
                })
            })
            .await
            .map_err(|e| failed("internal", &e.to_string()))?
    }
}

fn failed(code: &str, reason: &str) -> CoreError {
    CoreError::Failed { code: code.into(), reason: reason.into() }
}
