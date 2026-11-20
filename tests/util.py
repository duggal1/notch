import json


def jsonify(obj, **kwargs):
    """Serialize a Python object to a JSON string."""
    return json.dumps(obj, **kwargs)
