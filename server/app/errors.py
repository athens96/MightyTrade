from __future__ import annotations

from flask import Flask, jsonify
from werkzeug.exceptions import HTTPException


class ApiError(Exception):
    """Raise from anywhere to return a JSON error: {"error": code, "message": ...}."""

    def __init__(self, status: int, code: str, message: str = ""):
        super().__init__(message or code)
        self.status = status
        self.code = code
        self.message = message or code

    def __reduce__(self):
        return (type(self), (self.status, self.code, self.message))


def register_error_handlers(app: Flask) -> None:
    @app.errorhandler(ApiError)
    def _api_error(e: ApiError):
        return jsonify(error=e.code, message=e.message), e.status

    @app.errorhandler(HTTPException)
    def _http_error(e: HTTPException):
        return jsonify(error=e.name.lower().replace(" ", "_"), message=e.description), e.code

    @app.errorhandler(NotImplementedError)
    def _not_impl(e: NotImplementedError):
        return jsonify(error="not_implemented", message=str(e) or "Not implemented yet"), 501
