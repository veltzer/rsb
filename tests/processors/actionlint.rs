test_checker!(actionlint, tool: "actionlint", processor: "actionlint",
    files: [("ci.yml", "name: CI\non: push\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hello\n")]);
