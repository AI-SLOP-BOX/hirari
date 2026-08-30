use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HelpArticle { pub id: String, pub locale: String, pub title: String, pub body: String, pub keywords: Vec<String> }

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct HelpCatalog { pub articles: Vec<HelpArticle> }
impl HelpCatalog {
    pub fn search(&self, locale: &str, query: &str) -> Vec<HelpArticle> { let q=query.trim().to_ascii_lowercase(); let locale=locale.trim(); let matches = |a: &&HelpArticle, wanted: &str| a.locale.eq_ignore_ascii_case(wanted) && (q.is_empty()||a.title.to_ascii_lowercase().contains(&q)||a.body.to_ascii_lowercase().contains(&q)||a.keywords.iter().any(|k|k.to_ascii_lowercase().contains(&q))); let mut out: Vec<_>=self.articles.iter().filter(|a| matches(a, locale)).cloned().collect(); if out.is_empty() && !locale.eq_ignore_ascii_case("en") { out=self.articles.iter().filter(|a|matches(a, "en")).cloned().collect(); } out.sort_by(|a,b| a.title.to_ascii_lowercase().cmp(&b.title.to_ascii_lowercase()).then(a.id.cmp(&b.id))); out }
    pub fn validate(&self) -> bool { self.articles.len()<=65_536 && self.articles.iter().all(|a| !a.id.trim().is_empty()&&!a.id.contains('\0')&&!a.locale.trim().is_empty()&&a.locale.len()<=16&&!a.locale.contains('\0')&&!a.title.trim().is_empty()&&a.title.len()<=256&&!a.title.contains('\0')&&a.body.len()<=1_048_576&&!a.body.contains('\0')&&a.keywords.len()<=256&&a.keywords.iter().all(|k| !k.trim().is_empty()&&k.len()<=128&&!k.contains('\0'))) && self.articles.iter().enumerate().all(|(i,a)| self.articles[..i].iter().all(|p| p.id != a.id || !p.locale.eq_ignore_ascii_case(&a.locale))) }
}

#[cfg(test)]
mod tests { use super::*; #[test] fn falls_back_to_english() { let c=HelpCatalog{articles:vec![HelpArticle{id:"x".into(),locale:"en".into(),title:"Mixer".into(),body:"help".into(),keywords:vec!["mix".into()]}]}; assert_eq!(c.search("ja","mix").len(),1); } }
