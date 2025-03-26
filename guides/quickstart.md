# Quickstart: Deploy with MultiTool

This guide shows you how to deploy your own AWS Lambda code using MultiTool. You'll connect the CLI to your MultiTool dashboard and run a deployment using real application data.

## ✅ Prerequisites

- [ ] <a href="https://app.multitool.run/create-account" target="_blank">Create a free MultiTool account</a>

- [ ] Create a new workspace from the MultiTool web dashboard

- [ ] Create a new application in your workspace

## ⚙️ Install the MultiTool CLI

You can install the CLI using `curl`, <a href="https://brew.sh/" target="_blank">Homebrew</a>, or by downloading a binary from the <a href="https://github.com/wack/multitool/releases/latest" target="_blank">releases page</a>.

### Install with `curl`

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/wack/multitool/releases/download/v0.1.1/multitool-installer.sh | sh
```

### Install with <a href="https://brew.sh/" target="_blank">`Homebrew`</a>

```bash
brew install wack/tap/multi
```

## 🔐 Login with the MultiTool CLI

Connect the CLI to your MultiTool account:

```bash
multi login
```

## 🚀 Deploy your Lambda code

Once you have a `.zip` file containing your Lambda code, run:

```bash
multi run --workspace MY_WORKSPACE_NAME --application MY_APPLICATION_NAME my_code.zip
```

Replace:

- `MY_WORKSPACE_NAME` with the name of your MultiTool workspace

- `MY_APPLICATION_NAME` with the name of your application

- `my_code.zip` with the path to your build artifact

## 📬 Need help?

If you have questions, ideas, or bugs to report:

👉 [support@multitool.run](mailto:support@multitool.run)
