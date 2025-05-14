use serde::Deserialize;

#[derive(Deserialize)]
pub struct MetricsResponse {
    #[serde(default)]
    pub calculations: Vec<Calculation>,
}

#[derive(Deserialize, Clone)]
pub struct Calculation {
    #[serde(default)]
    pub aggregates: Vec<Aggregate>,
}

#[derive(Deserialize, Clone)]
pub struct Aggregate {
    #[serde(default)]
    pub count: u32,
}
