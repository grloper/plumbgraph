from app.main import app


@app.route("/x")
def view():
    return "x"
