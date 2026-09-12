/// Host-neutral articulation/expression-map entry. Program and channel are
/// encoded exactly as MIDI values; transpose is a bounded semitone offset.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Articulation {
    pub name: String,
    pub program: u8,
    pub transpose: i16,
    pub channel: u8,
}

impl Articulation {
    pub fn validate(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.len() <= 128
            && !self.name.contains('\0')
            && self.channel < 16
            && self.transpose.abs() <= 48
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExpressionMap {
    pub name: String,
    pub articulations: Vec<Articulation>,
}

impl ExpressionMap {
    pub fn validate(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.len() <= 128
            && !self.name.contains('\0')
            && self.articulations.len() <= 1024
            && self.articulations.iter().all(Articulation::validate)
            && self.articulations.iter().enumerate().all(|(index, item)| {
                self.articulations[..index]
                    .iter()
                    .all(|previous| !previous.name.eq_ignore_ascii_case(&item.name))
            })
    }

    pub fn upsert(&mut self, mut articulation: Articulation) -> bool {
        if !articulation.validate() {
            return false;
        }
        articulation.name = articulation.name.trim().to_owned();
        if let Some(existing) = self
            .articulations
            .iter_mut()
            .find(|item| item.name.eq_ignore_ascii_case(&articulation.name))
        {
            *existing = articulation;
        } else if self.articulations.len() < 1024 {
            self.articulations.push(articulation);
        } else {
            return false;
        }
        self.articulations
            .sort_by_key(|item| item.name.to_ascii_lowercase());
        true
    }

    pub fn find(&self, name: &str) -> Option<&Articulation> {
        self.articulations
            .iter()
            .find(|item| item.name.eq_ignore_ascii_case(name.trim()))
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.articulations.len();
        self.articulations
            .retain(|item| !item.name.eq_ignore_ascii_case(name.trim()));
        before != self.articulations.len()
    }

    pub fn search(&self, query: &str) -> Vec<Articulation> {
        let query = query.trim().to_ascii_lowercase();
        let mut result: Vec<_> = self
            .articulations
            .iter()
            .filter(|item| query.is_empty() || item.name.to_ascii_lowercase().contains(&query))
            .cloned()
            .collect();
        result.sort_by_key(|item| item.name.to_ascii_lowercase());
        result
    }

    /// Resolves the MIDI channel and transposed note for a selected program.
    pub fn resolve(&self, name: &str, note: u8) -> Option<(u8, u8, u8)> {
        let articulation = self.find(name)?;
        let transposed = i16::from(note)
            .checked_add(articulation.transpose)?
            .clamp(0, 127) as u8;
        Some((articulation.channel, articulation.program, transposed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_and_manages_articulations_case_insensitively() {
        let mut map = ExpressionMap {
            name: "Strings".into(),
            articulations: Vec::new(),
        };
        assert!(map.upsert(Articulation {
            name: "Legato ".into(),
            program: 4,
            transpose: 1,
            channel: 2
        }));
        assert_eq!(map.resolve("LEGATO", 60), Some((2, 4, 61)));
        assert_eq!(map.search("leg").len(), 1);
        assert!(map.remove("legato"));
        assert!(map.validate());
    }
}
