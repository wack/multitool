# MultiTool Quickstart

## Prerequisites

- [ ] <a href="https://app.multitool.run/create-account" target="_blank">Create a free MultiTool account.</a>

- [ ] Create a new Workspace in MultiTool.

- [ ] Create a new Application in MultiTool.

## Install the MultiTool CLI

To install the MultiTool CLI, use `curl`, <a href="https://brew.sh/" target="_blank">Homebrew</a>, or <a href="https://github.com/wack/multitool/releases/latest" target="_blank">vist our releases page</a> to download a pre-built binary .

With `curl`

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/wack/multitool/releases/download/v0.1.1/multitool-installer.sh | sh
```

<a href="https://brew.sh/" target="_blank">With `Homebrew`</a>

```bash
brew install wack/tap/multi
```

## Login with the MultiTool CLI

Run the login command to connect the MultiTool CLI with your dashboard.

```bash
multi login
```

## Deploy code using MultiTool

Once you have a build artifact (zip file of Lambda code) that you would like to deploy, use the `multi run` command, passing in the Workspace name and Application name that you created during setup.

```bash
multi run --workspace MY_WORKSPACE_NAME --application MY_APPLICATION_NAME my_code.zip
```

## 📬 Need help?

If you have questions, ideas, or bugs to report:

👉 [support@multitool.run](mailto:support@multitool.run)!
