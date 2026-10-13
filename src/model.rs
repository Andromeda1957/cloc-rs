use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Counts {
    #[serde(rename = "nFiles", default)]
    pub files: u64,
    #[serde(default)]
    pub blank: u64,
    #[serde(default)]
    pub comment: u64,
    #[serde(default)]
    pub code: u64,
}
impl Counts {
    pub fn add(&mut self, o: Self) {
        self.files += o.files;
        self.blank += o.blank;
        self.comment += o.comment;
        self.code += o.code;
    }
    pub fn lines(self) -> u64 {
        self.blank + self.comment + self.code
    }
    pub fn line(&mut self, k: Kind) {
        match k {
            Kind::Blank => self.blank += 1,
            Kind::Comment => self.comment += 1,
            Kind::Code => self.code += 1,
            Kind::Ignored => (),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Blank,
    Comment,
    Code,
    Ignored,
}
#[derive(Debug, Clone)]
pub struct Source {
    pub name: String,
    pub key: String,
    pub bytes: Vec<u8>,
    pub explicit: bool,
}
#[derive(Debug, Clone)]
pub struct FileCount {
    pub name: String,
    pub key: String,
    pub language: String,
    pub counts: Counts,
    pub lines: Vec<String>,
    pub kinds: Vec<Kind>,
    pub stripped: Vec<String>,
}
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub files: Vec<FileCount>,
    pub languages: BTreeMap<String, Counts>,
    pub total: Counts,
    pub found: Vec<String>,
    pub ignored: Vec<(String, String)>,
    pub elapsed: f64,
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct DiffCounts {
    pub added: Counts,
    pub removed: Counts,
    pub modified: Counts,
    pub same: Counts,
}
impl DiffCounts {
    pub fn add(&mut self, o: &Self) {
        self.added.add(o.added);
        self.removed.add(o.removed);
        self.modified.add(o.modified);
        self.same.add(o.same);
    }
}
#[derive(Debug, Clone, Default)]
pub struct DiffReport {
    pub languages: BTreeMap<String, DiffCounts>,
    pub files: BTreeMap<String, (String, DiffCounts)>,
    pub total: DiffCounts,
    pub alignment: Vec<String>,
    pub elapsed: f64,
}
