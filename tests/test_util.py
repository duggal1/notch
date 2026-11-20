from util import jsonify


def test_jsonify():
    assert jsonify({"a": 1}) == '{"a": 1}'
