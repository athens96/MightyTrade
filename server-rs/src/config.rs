use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

fn env_bool(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(v) => matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => default,
    }
}

fn env_csv(name: &str) -> Vec<String> {
    env::var(name)
        .unwrap_or_default()
        .split(',')
        .map(|v| v.trim().to_lowercase())
        .filter(|v| !v.is_empty())
        .collect()
}

fn env_i64(name: &str, default: i64) -> i64 {
    env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn env_f64(name: &str, default: f64) -> f64 {
    env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

#[derive(Debug, Clone)]
pub struct Config {
    pub env_mode: String,
    pub debug: bool,
    pub testing: bool,
    pub bind_addr: String,

    pub secret_key: String,
    pub encryption_key: String,

    pub google_client_id: String,
    pub allowed_emails: Vec<String>,
    pub admin_emails: Vec<String>,

    pub otp_issuer: String,
    pub otp_max_fails: usize,
    pub otp_lock_min: i64,

    pub access_token_ttl_min: i64,
    pub pending_token_ttl_min: i64,
    pub cookie_secure: bool,

    pub storage_backend: String,
    pub data_dir: PathBuf,

    pub market_providers: HashMap<String, String>,

    pub toss_api_base_url: String,
    pub toss_timeout_sec: f64,

    pub trading_unlock_min: i64,
    pub trading_max_orders_per_day: usize,
    pub trading_max_order_krw: f64,
    pub trading_max_order_usd: f64,

    pub autotrade_scheduler: bool,
    pub quant_workers: usize,
}

impl Config {
    pub fn from_env() -> Self {
        let env_mode = env::var("APP_ENV")
            .or_else(|_| env::var("FLASK_ENV"))
            .unwrap_or_else(|_| "development".to_string())
            .trim()
            .to_lowercase();
        let is_prod = env_mode == "production";
        let is_test = env_mode == "testing";

        let base_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let default_data = base_dir.join("data");
        let data_dir = env::var("DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or(default_data);

        let mut market_providers = HashMap::new();
        market_providers.insert(
            "KR".to_string(),
            env::var("MARKET_PROVIDER_KR")
                .unwrap_or_else(|_| "mock".to_string())
                .trim()
                .to_lowercase(),
        );
        market_providers.insert(
            "US".to_string(),
            env::var("MARKET_PROVIDER_US")
                .unwrap_or_else(|_| "mock".to_string())
                .trim()
                .to_lowercase(),
        );

        let host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port = env::var("PORT").unwrap_or_else(|_| "5001".to_string());
        let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| format!("{host}:{port}"));

        Self {
            env_mode,
            debug: !is_prod && !is_test,
            testing: is_test,
            bind_addr,
            secret_key: env::var("SECRET_KEY")
                .unwrap_or_else(|_| "dev-insecure-secret-do-not-use-in-prod".to_string()),
            encryption_key: env::var("ENCRYPTION_KEY").unwrap_or_default(),
            google_client_id: env::var("GOOGLE_CLIENT_ID").unwrap_or_default(),
            allowed_emails: env_csv("ALLOWED_EMAILS"),
            admin_emails: env_csv("ADMIN_EMAILS"),
            otp_issuer: env::var("OTP_ISSUER").unwrap_or_else(|_| "MightyTrade".to_string()),
            otp_max_fails: env_usize("OTP_MAX_FAILS", 5),
            otp_lock_min: env_i64("OTP_LOCK_MIN", 5),
            access_token_ttl_min: env_i64("ACCESS_TOKEN_TTL_MIN", 720),
            pending_token_ttl_min: env_i64("PENDING_TOKEN_TTL_MIN", 10),
            cookie_secure: env_bool("COOKIE_SECURE", is_prod),
            storage_backend: env::var("STORAGE_BACKEND")
                .unwrap_or_else(|_| "json".to_string())
                .trim()
                .to_lowercase(),
            data_dir,
            market_providers,
            toss_api_base_url: env::var("TOSS_API_BASE_URL")
                .unwrap_or_else(|_| "https://openapi.tossinvest.com".to_string()),
            toss_timeout_sec: env_f64("TOSS_TIMEOUT_SEC", 10.0),
            trading_unlock_min: env_i64("TRADING_UNLOCK_MIN", 30),
            trading_max_orders_per_day: env_usize("TRADING_MAX_ORDERS_PER_DAY", 50),
            trading_max_order_krw: env_f64("TRADING_MAX_ORDER_KRW", 50_000_000.0),
            trading_max_order_usd: env_f64("TRADING_MAX_ORDER_USD", 50_000.0),
            autotrade_scheduler: if is_test {
                false
            } else {
                env_bool("AUTOTRADE_SCHEDULER", true)
            },
            quant_workers: if is_test { 0 } else { env_usize("QUANT_WORKERS", 2) },
        }
    }

    pub fn validate_production(&self) -> Result<(), String> {
        if !self.debug && !self.testing {
            if self.secret_key == "dev-insecure-secret-do-not-use-in-prod" {
                return Err("SECRET_KEY must be set in production".to_string());
            }
            if self.encryption_key.is_empty() {
                return Err("ENCRYPTION_KEY must be set in production".to_string());
            }
        }
        Ok(())
    }

    pub fn test_default(data_dir: PathBuf) -> Self {
        let mut market_providers = HashMap::new();
        market_providers.insert("KR".to_string(), "mock".to_string());
        market_providers.insert("US".to_string(), "mock".to_string());
        Self {
            env_mode: "testing".to_string(),
            debug: false,
            testing: true,
            bind_addr: "127.0.0.1:0".to_string(),
            secret_key: "test-secret-key-for-unit-tests".to_string(),
            encryption_key: String::new(),
            google_client_id: "test-google-client-id".to_string(),
            allowed_emails: vec![],
            admin_emails: vec!["admin@example.com".to_string()],
            otp_issuer: "MightyTrade".to_string(),
            otp_max_fails: 5,
            otp_lock_min: 5,
            access_token_ttl_min: 720,
            pending_token_ttl_min: 10,
            cookie_secure: false,
            storage_backend: "sqlite".to_string(),
            data_dir,
            market_providers,
            toss_api_base_url: "https://openapi.tossinvest.com".to_string(),
            toss_timeout_sec: 10.0,
            trading_unlock_min: 30,
            trading_max_orders_per_day: 50,
            trading_max_order_krw: 50_000_000.0,
            trading_max_order_usd: 50_000.0,
            autotrade_scheduler: false,
            quant_workers: 0,
        }
    }
}
