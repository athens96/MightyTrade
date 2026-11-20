import pytest

from app import create_app

# Tests must not depend on the developer's local .env (real keys, providers, allowlist).
TEST_OVERRIDES = {
    "GOOGLE_CLIENT_ID": "test-client",
    "ALLOWED_EMAILS": [],
    "MARKET_PROVIDERS": {"KR": "mock", "US": "mock"},
    "TOSS_API_BASE_URL": "https://toss.test",
    "ENCRYPTION_KEY": "",
}


@pytest.fixture
def app(tmp_path):
    return create_app("testing", {**TEST_OVERRIDES, "DATA_DIR": tmp_path})


@pytest.fixture
def client(app):
    return app.test_client()
