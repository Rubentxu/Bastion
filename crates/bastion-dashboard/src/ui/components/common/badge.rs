//! Badge component for displaying sandbox purposes and other labels.

/// Sandbox purpose types that map to specific colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    AdHocTest,
    ProofOfConcept,
    E2eTest,
    RealTest,
    PipelineStage,
    Job,
}

impl Purpose {
    /// Parse purpose from a string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "adhoc_test" | "adhottest" => Some(Purpose::AdHocTest),
            "proof_of_concept" | "proofofconcept" | "poc" => Some(Purpose::ProofOfConcept),
            "e2e_test" | "e2etest" => Some(Purpose::E2eTest),
            "real_test" | "realtest" => Some(Purpose::RealTest),
            "pipeline_stage" | "pipelinestage" => Some(Purpose::PipelineStage),
            "job" => Some(Purpose::Job),
            _ => None,
        }
    }

    /// Get the display label for this purpose.
    pub fn label(&self) -> &'static str {
        match self {
            Purpose::AdHocTest => "AdHocTest",
            Purpose::ProofOfConcept => "ProofOfConcept",
            Purpose::E2eTest => "E2eTest",
            Purpose::RealTest => "RealTest",
            Purpose::PipelineStage => "PipelineStage",
            Purpose::Job => "Job",
        }
    }

    /// Get the Tailwind CSS classes for this purpose's color.
    pub fn color_classes(&self) -> &'static str {
        match self {
            Purpose::AdHocTest => "bg-yellow-100 text-yellow-800 border-yellow-300",
            Purpose::ProofOfConcept => "bg-blue-100 text-blue-800 border-blue-300",
            Purpose::E2eTest => "bg-green-100 text-green-800 border-green-300",
            Purpose::RealTest => "bg-orange-100 text-orange-800 border-orange-300",
            Purpose::PipelineStage => "bg-purple-100 text-purple-800 border-purple-300",
            Purpose::Job => "bg-gray-100 text-gray-800 border-gray-300",
        }
    }
}
