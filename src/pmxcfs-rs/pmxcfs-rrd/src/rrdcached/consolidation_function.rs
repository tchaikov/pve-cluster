#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConsolidationFunction {
    Average,
    Min,
    Max,
    Last,
}

impl ConsolidationFunction {
    pub fn to_str(self) -> &'static str {
        match self {
            ConsolidationFunction::Average => "AVERAGE",
            ConsolidationFunction::Min => "MIN",
            ConsolidationFunction::Max => "MAX",
            ConsolidationFunction::Last => "LAST",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_consolidation_function_to_str() {
        assert_eq!(ConsolidationFunction::Average.to_str(), "AVERAGE");
        assert_eq!(ConsolidationFunction::Min.to_str(), "MIN");
        assert_eq!(ConsolidationFunction::Max.to_str(), "MAX");
        assert_eq!(ConsolidationFunction::Last.to_str(), "LAST");
    }
}
