<div align="center">
    <a href="https://www.botanixlabs.xyz/en/home">
        <img src="./images/botanix.jpg" alt="Logo" style="transform: scale(0.7);">
    </a>
    <h1 align="center">Botanix Protocol</h1>
    <p align="center">
        A blazing fast and secure L2 for Bitcoin using the EVM as a superstructure.
    </p>
    <p align="center">
        <a href="https://github.com/botanix-labs/Macbeth/actions/workflows/unit.yml" style="text-decoration: none;">
            <img src="https://github.com/botanix-labs/Macbeth/actions/workflows/unit.yml/badge.svg?branch=fix/rwlock/coverage" alt="Botanix Unit Tests">
        </a>
        <a href="https://github.com/botanix-labs/Macbeth/actions/workflows/integration.yml" style="text-decoration: none;">
            <img src="https://github.com/botanix-labs/Macbeth/actions/workflows/integration.yml/badge.svg?branch=fix/rwlock/coverage" alt="Botanix Integration Tests">
        </a>
         <a href="https://codecov.io/github/botanix-labs/Macbeth/" style="text-decoration: none;">
          <img src="https://codecov.io/github/botanix-labs/Macbeth/graph/badge.svg?token=V1NH5QV17M" alt="Coverage"/>
        </a>
        <a href="https://github.com/botanix-labs/Macbeth/blob/main/LICENSE-MIT" style="text-decoration: none;">
            <img src="https://img.shields.io/badge/license-MIT-007EC7.svg" alt="License">
        </a>
    </p>
</div>

# Running and Testing the Project

1.  [Setting up nodes locally](./docs/local_setup.md)
2.  [Setting up nodes locally using scripts](./docs/local_setup_with_scripts.md)
3.  [Running nodes with Docker](./docs/docker_setup.md)
4.  [Executing the test suite](./docs/test-suite.md)

## Getting Help

If you have any questions, first see if the answer to your question can be found in the [book].

[book]: https://docs.botanixlabs.xyz/botanix-labs/

If the answer is not there:

-   Join the [Telegram](https://botanixlabs.xyz/en/home) to get help, or
-   Open a [discussion](https://github.com/botanix-labs/Macbeth/issues/new) with your question, or
-   Open an issue with [the bug](https://github.com/botanix-labs/Macbeth/issues)

## Submitting a Pull Request

To ensure code quality and consistency, please follow these steps when preparing to submit a pull request:

1. **Install Pre-commit**:
   Make sure you have [pre-commit](https://pre-commit.com/) installed on your machine. This tool helps enforce code formatting and other checks before committing changes.

2. **Install Dependencies**:
   Run `pnpm i` in the root directory (this step only needs to be done once). Make sure you have `node` and `pnpm` installed on your system.

3. **Format Code Before Pushing**:
   Anytime you've made changes and are ready to push, run `make fmt`. This will format your code according to the project's standards.

4. **Run Lint Checks (Optional but Recommended)**:
   Occasionally, you may want to perform a lint check by running `make lint`. This will run additional checks (like `clippy` for Rust) to catch potential improvements. Note that lint checks are not automatically enforced by the pipeline, so it is your responsibility to ensure the code quality is maintained and that `clippy` does not flag any issues.

---

By following these steps, you help ensure that all code contributions meet the project's quality standards. Thank you!

## Security

See [`SECURITY.md`](./SECURITY.md).
