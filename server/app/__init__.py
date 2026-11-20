from __future__ import annotations

from dotenv import load_dotenv
from flask import Flask, jsonify

load_dotenv()

from .config import get_config  # noqa: E402  (env must be loaded first)
from .errors import register_error_handlers  # noqa: E402


def create_app(config_name: str | None = None, overrides: dict | None = None) -> Flask:
    app = Flask(__name__)
    app.config.from_object(get_config(config_name))
    if overrides:
        app.config.update(overrides)

    if not app.debug and not app.testing:
        if app.config["SECRET_KEY"] == "dev-insecure-secret-do-not-use-in-prod":
            raise RuntimeError("SECRET_KEY must be set in production")
        if not app.config["ENCRYPTION_KEY"]:
            raise RuntimeError("ENCRYPTION_KEY must be set in production")

    from .storage import init_storage
    init_storage(app)

    from .auth.routes import bp as auth_bp
    from .market.routes import bp as market_bp
    from .trading.routes import bp as trading_bp
    from .users.routes import bp as users_bp

    app.register_blueprint(auth_bp, url_prefix="/api/auth")
    app.register_blueprint(users_bp, url_prefix="/api/users")
    app.register_blueprint(market_bp, url_prefix="/api/market")
    app.register_blueprint(trading_bp, url_prefix="/api/trading")

    register_error_handlers(app)

    @app.get("/api/health")
    def health():
        return jsonify(status="ok")

    return app
