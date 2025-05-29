use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct MetricsResponse {
    #[serde(default)]
    pub calculations: Vec<Calculation>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Calculation {
    #[serde(default)]
    pub aggregates: Vec<Aggregate>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Aggregate {
    #[serde(default)]
    pub count: u32,
}
