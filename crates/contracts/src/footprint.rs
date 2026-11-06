//! Canonical maker footprint (#894, PRD #878 F1).

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Footprint(Vec<String>);

impl Footprint {
    pub fn new(paths: Vec<String>) -> Result<Self, String> {
        Ok(Self(paths))
    }

    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalises_and_deduplicates_declared_paths() {
        let footprint = Footprint::new(
            ["./x.py", "a//b.py", "dir/", "a/./b.py", "x.py"]
                .map(String::from)
                .to_vec(),
        ).unwrap();
        assert_eq!(footprint.as_slice(), ["x.py", "a/b.py", "dir"]);
    }

    #[test]
    fn refuses_paths_that_cannot_name_repository_files() {
        for path in ["..", "a/../b.py", "/x.py", "//x.py", "", ".", "./", "x\0.py", "C:\\x.py", ".git/config"] {
            assert!(Footprint::new(vec![path.into()]).is_err(), "accepted {path:?}");
        }
    }

    #[test]
    fn maker_input_exposes_a_required_canonical_footprint() {
        let schema = crate::roles::maker_input::schema();
        assert_eq!(schema["properties"]["footprint"]["type"], "array");
        assert!(schema["required"].as_array().unwrap().contains(&serde_json::json!("footprint")));
    }
}
