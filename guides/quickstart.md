# Quickstart: Deploy with MultiTool

This guide shows you how to deploy your own AWS Lambda code using MultiTool. You'll connect the CLI to your MultiTool dashboard and run a deployment using Lambda code packaged as a zip file.

## ✅ Prerequisites

- [ ] <a href="https://app.multitool.run/create-account" target="_blank">Create a free MultiTool account</a>

- [ ] Create a new workspace from the MultiTool app

- [ ] Create a new application in your workspace

## ⚙️ Install the MultiTool CLI

You can install the CLI using `curl`, <a href="https://brew.sh/" target="_blank">Homebrew</a>, or by downloading a binary from the <a href="https://github.com/wack/multitool/releases/latest" target="_blank">releases page</a>.

### Install with `curl`

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/wack/multitool/releases/download/v0.3.0/multitool-installer.sh | sh
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

## 📜 Create a manifest file

For Cloudflare:

**`MultiTool.toml`**

```toml
# Your workspace's name
workspace = ""
# Your application's name
application = ""

[config.cloudflare]
# The name of your worker
worker-name = ""
# Your Cloudflare account id
account-id = ""
# The path to the directory where your main-module is
artifact-path = ""
# The main module of your function
main-module = ".js"
```

For AWS Lambda:

**`MultiTool.toml`**

```toml
# Your workspace's name
workspace = ""
# Your application's name
application = ""

config.monitor.aws-cloudwatch = {}

[config.platform.aws-lambda]
# The name of your Lambda function
name = ""
# The AWS Region
region = ""
# The path to the zip file of your Lambda's code
artifact-path = ".zip"

[config.ingress.aws-api-gateway]
# The name of your API Gateway
gateway-name = ""
# The name of your API Gateway's stage
stage-name = ""
# The resource path of your API Gateway (including the leading slash)
resource-path = ""
# The resource method of your API Gateway
resource-method = ""
# The AWS Region
region = ""
```

## 🚀 Deploy your artifact

Start your rollout!

```bash
multi run --cloudflare-api-token MY_CLOUDFLARE_TOKEN
```

Or if you're deploying to AWS:

```bash
multi run
```

## 📬 Need help?

If you have questions, ideas, or bugs to report:

👉 [support@multitool.run](mailto:support@multitool.run)
