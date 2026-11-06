//! Project organization only: stable bins and sparse asset metadata. No paths or I/O.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::model::{AssetId, MediaAsset};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BinId(pub u64);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bin: Option<BinId>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub favorite: bool,
}

/// Absent metadata means the flat, unfiled, non-favorite default. Asset IDs never change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaLibrary {
    bins: BTreeMap<BinId, String>,
    entries: BTreeMap<AssetId, LibraryEntry>,
    next_bin: u64,
}

/// Applied through History as a single undo step, including multi-selection edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryEdit {
    CreateBin {
        name: String,
    },
    RenameBin {
        id: BinId,
        name: String,
    },
    /// Unfiles members, keeping the assets, favorites, source files and timeline clips.
    DeleteBin(BinId),
    /// `None` moves the items back to the flat, unfiled view.
    MoveAssets {
        ids: Vec<AssetId>,
        bin: Option<BinId>,
    },
    SetFavorites {
        ids: Vec<AssetId>,
        favorite: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryError {
    UnknownBin(BinId),
    UnknownAsset(AssetId),
    EmptyName,
    DuplicateName,
    BinLimit,
}

impl std::fmt::Display for LibraryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownBin(id) => write!(f, "unknown media bin {}", id.0),
            Self::UnknownAsset(id) => write!(f, "unknown asset {}", id.0),
            Self::EmptyName => f.write_str("media bin name is empty"),
            Self::DuplicateName => f.write_str("media bin name already exists"),
            Self::BinLimit => f.write_str("media bin identifiers exhausted"),
        }
    }
}

impl std::error::Error for LibraryError {}

impl MediaLibrary {
    pub fn is_empty(&self) -> bool {
        // Keep the ID high-water mark after deleting the final bin, including across reopen.
        self.bins.is_empty() && self.entries.is_empty() && self.next_bin == 0
    }

    pub fn bins(&self) -> &BTreeMap<BinId, String> {
        &self.bins
    }

    pub fn entry(&self, id: AssetId) -> LibraryEntry {
        self.entries.get(&id).copied().unwrap_or_default()
    }

    pub fn check_name(&self, name: &str, except: Option<BinId>) -> Result<(), LibraryError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(LibraryError::EmptyName);
        }
        if self
            .bins
            .iter()
            .any(|(id, n)| Some(*id) != except && n.to_lowercase() == name.to_lowercase())
        {
            return Err(LibraryError::DuplicateName);
        }
        Ok(())
    }

    /// Validates all inputs before mutation, so a failed edit leaves metadata unchanged.
    pub(crate) fn apply(
        &mut self,
        edit: LibraryEdit,
        assets: &[MediaAsset],
    ) -> Result<(), LibraryError> {
        match edit {
            LibraryEdit::CreateBin { name } => {
                self.check_name(&name, None)?;
                let highest = self.bins.keys().next_back().map_or(0, |id| id.0);
                let next = self
                    .next_bin
                    .max(highest)
                    .checked_add(1)
                    .ok_or(LibraryError::BinLimit)?;
                self.bins.insert(BinId(next), name.trim().into());
                self.next_bin = next;
            }
            LibraryEdit::RenameBin { id, name } => {
                self.require_bin(id)?;
                self.check_name(&name, Some(id))?;
                self.bins.insert(id, name.trim().into());
            }
            LibraryEdit::DeleteBin(id) => {
                self.require_bin(id)?;
                self.bins.remove(&id);
                self.entries.retain(|_, entry| {
                    if entry.bin == Some(id) {
                        entry.bin = None;
                    }
                    *entry != LibraryEntry::default()
                });
            }
            LibraryEdit::MoveAssets { ids, bin } => {
                if let Some(id) = bin {
                    self.require_bin(id)?;
                }
                Self::require_assets(&ids, assets)?;
                for id in ids {
                    self.update(id, |entry| entry.bin = bin);
                }
            }
            LibraryEdit::SetFavorites { ids, favorite } => {
                Self::require_assets(&ids, assets)?;
                for id in ids {
                    self.update(id, |entry| entry.favorite = favorite);
                }
            }
        }
        Ok(())
    }

    fn require_bin(&self, id: BinId) -> Result<(), LibraryError> {
        self.bins
            .contains_key(&id)
            .then_some(())
            .ok_or(LibraryError::UnknownBin(id))
    }

    fn require_assets(ids: &[AssetId], assets: &[MediaAsset]) -> Result<(), LibraryError> {
        let known: HashSet<_> = assets.iter().map(|a| a.id).collect();
        match ids.iter().find(|id| !known.contains(id)) {
            Some(id) => Err(LibraryError::UnknownAsset(*id)),
            None => Ok(()),
        }
    }

    fn update(&mut self, id: AssetId, change: impl FnOnce(&mut LibraryEntry)) {
        let mut entry = self.entry(id);
        change(&mut entry);
        if entry == LibraryEntry::default() {
            self.entries.remove(&id);
        } else {
            self.entries.insert(id, entry);
        }
    }

    pub(crate) fn forget_asset(&mut self, id: AssetId) {
        self.entries.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        file,
        history::{Command, Created, History},
        model::{NewClip, Project, TrackKind},
        time::Time,
    };
    use std::path::Path;

    fn edit(h: &mut History, edit: LibraryEdit) {
        h.apply(Command::EditMediaLibrary(edit)).unwrap();
    }

    #[test]
    fn organization_rename_delete_undo_and_reopen_keep_media_and_clips() {
        let mut h = History::new(Project::default());
        h.apply(Command::AddAsset("/media/original.mov".into()))
            .unwrap();
        let Created::Track(track) = h
            .apply(Command::AddTrack {
                kind: TrackKind::Video,
                name: "V1".into(),
            })
            .unwrap()
        else {
            panic!()
        };
        h.apply(Command::AddClip {
            track,
            clip: NewClip {
                asset: AssetId(1),
                source_in: Time(0),
                source_out: Time(10),
                start: Time(0),
                link: None,
            },
        })
        .unwrap();
        let flat = h.project().clone();
        edit(
            &mut h,
            LibraryEdit::CreateBin {
                name: " Footage ".into(),
            },
        );
        edit(
            &mut h,
            LibraryEdit::MoveAssets {
                ids: vec![AssetId(1)],
                bin: Some(BinId(1)),
            },
        );
        edit(
            &mut h,
            LibraryEdit::SetFavorites {
                ids: vec![AssetId(1)],
                favorite: true,
            },
        );
        edit(
            &mut h,
            LibraryEdit::RenameBin {
                id: BinId(1),
                name: "Selects".into(),
            },
        );
        let organized = h.project().clone();
        assert_eq!(organized.assets(), flat.assets());
        assert_eq!(organized.sequence(), flat.sequence());
        assert_eq!(
            crate::eval::frame_at(&organized, Time(5)),
            crate::eval::frame_at(&flat, Time(5))
        );
        assert_eq!(organized.media_library().bins()[&BinId(1)], "Selects");
        edit(&mut h, LibraryEdit::DeleteBin(BinId(1)));
        assert_eq!(
            h.project().media_library().entry(AssetId(1)),
            LibraryEntry {
                bin: None,
                favorite: true
            }
        );
        assert_eq!(h.project().assets(), flat.assets());
        assert_eq!(
            crate::eval::frame_at(h.project(), Time(5)),
            crate::eval::frame_at(&flat, Time(5))
        );
        assert!(h.undo());
        assert_eq!(h.project(), &organized);
        let json = file::to_json(h.project(), Path::new("/project")).unwrap();
        let reopened = file::from_json(&json, Path::new("/project"), |_| false).unwrap();
        assert_eq!(&reopened, h.project());
        h.apply(Command::RemoveAsset(AssetId(1))).unwrap();
        assert_eq!(
            h.project().media_library().entry(AssetId(1)),
            LibraryEntry::default()
        );
        assert!(h.undo());
        assert_eq!(h.project(), &organized);
        assert!(h.redo());
        assert!(h.project().assets().is_empty());
    }

    #[test]
    fn invalid_edits_are_atomic_and_do_not_dirty_history() {
        let mut h = History::new(Project::default());
        h.apply(Command::AddAsset("clip.mov".into())).unwrap();
        edit(
            &mut h,
            LibraryEdit::CreateBin {
                name: "Selects".into(),
            },
        );
        h.mark_saved();
        let before = h.project().clone();
        for invalid in [
            LibraryEdit::CreateBin { name: "  ".into() },
            LibraryEdit::CreateBin {
                name: "SELECTS".into(),
            },
            LibraryEdit::RenameBin {
                id: BinId(99),
                name: "Other".into(),
            },
            LibraryEdit::DeleteBin(BinId(99)),
            LibraryEdit::MoveAssets {
                ids: vec![AssetId(1)],
                bin: Some(BinId(99)),
            },
            LibraryEdit::MoveAssets {
                ids: vec![AssetId(1), AssetId(99)],
                bin: Some(BinId(1)),
            },
            LibraryEdit::SetFavorites {
                ids: vec![AssetId(1), AssetId(99)],
                favorite: true,
            },
        ] {
            assert!(h.apply(Command::EditMediaLibrary(invalid)).is_err());
            assert_eq!(h.project(), &before);
            assert!(!h.is_dirty());
        }
    }

    #[test]
    fn legacy_flat_default_and_bin_ids_survive_reopen() {
        let mut h = History::new(Project::default());
        let json = file::to_json(h.project(), Path::new("/project")).unwrap();
        assert!(!json.contains("media_library"));
        let legacy = file::from_json(&json, Path::new("/project"), |_| false).unwrap();
        assert!(legacy.media_library().is_empty());
        edit(&mut h, LibraryEdit::CreateBin { name: "One".into() });
        edit(&mut h, LibraryEdit::DeleteBin(BinId(1)));
        let json = file::to_json(h.project(), Path::new("/project")).unwrap();
        let mut reopened =
            History::new(file::from_json(&json, Path::new("/project"), |_| false).unwrap());
        edit(&mut reopened, LibraryEdit::CreateBin { name: "Two".into() });
        assert!(
            reopened
                .project()
                .media_library()
                .bins()
                .contains_key(&BinId(2))
        );
    }
}
