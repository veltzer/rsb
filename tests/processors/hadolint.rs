test_checker!(hadolint, tool: "hadolint", processor: "hadolint",
    files: [("Dockerfile", "FROM ubuntu:22.04\nCMD [\"echo\", \"hello\"]\n")]);
