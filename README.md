<img align="center" width="1200" alt="gh-multi-banner-2" src="https://github.com/user-attachments/assets/43ec8b3f-9443-4b64-a737-906b82fd80f4" />

<h1 align="center">MultiTool</h1>
<p align="center"><b>Agentic deployments help teams catch production bugs before they impact users. Move fast without breaking things.</b></p>

<p align="center">
✨ <a href="https://app.multitool.run/create-account">Try the MultiTool beta for free</a> • 🏡 <a href="https://www.multitool.run/">MultiTool home</a> • ✍️ <a href="https://www.multitool.run/blog">The MultiTool Blog</a>

## ❓ What is MultiTool?

MultiTool is a progressive delivery tool that helps teams catch production bugs before they impact users. The open-source CLI connects to your <a href="https://app.multitool.run/create-account">MultiTool account</a> to create and monitor canary deployments, manage traffic shifting, and automatically roll back changes when statistically significant regressions are detected.

## 📖 Table of contents

- [What is MultiTool?](#-what-is-multitool)
- [Table of contents](#-table-of-contents)
- [Getting started](#%EF%B8%8F-getting-started)
- [Installation](#%EF%B8%8F-installation)
- [Features](#-features)
- [Mission](#-mission)
- [Need help?](#-need-help)

## 🏎️ Getting started

Check out our [quickstart guide](/guides/quickstart.md) to learn how to deploy with MultiTool!

Check out our [getting started tutorial](/guides/getting-started.md) to try out a guided demo of MultiTool!


## ⚙️ Installation

**Installing with Homebrew:**

```sh
brew install wack/tap/multi
```

**Installing with curl:**

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/wack/multitool/releases/download/v0.1.1/multitool-installer.sh | sh
```

Check the [releases page](https://github.com/wack/canary/releases) for pre-built binaries, checksums, and guides to install on additional platforms.

## ⭐ Features

### Automated canary deployments
The MultiTool agent deploys every release as a canary deployment, routing a small percentage of production traffic to the canary.

### Traffic scale-up
As the agent gains confidence in the release, it automatically scales up traffic. No more watching logs or waiting for alerts.

### Fast roll-backs
The MultiTool agent automatically rolls back the deployment if it detects a statistical increase in errors, preventing user-facing bugs and downtime. MultiTool keeps a hot standby ready for instantaneous rollback in case of incident.

### Run locally or in CI/CD
MultiTool doesn’t store your cloud provider keys. The MultiTool agent runs on your local system or any CI/CD tool you currently use.

## 🛠️ Platform support

MultiTool supports deploying canaries for AWS Lambda functions running in AWS API Gateway. The MultiTool team is actively expanding platform support. Our platform roadmap is presented in the following table:

|                           Platform                           |            Support             |
| :----------------------------------------------------------: | :----------------------------: |
|                 **AWS Lambda + API Gateway**                 |     :sparkles: Available!      |
|                     [Vercel](vercel.com)                     |         :eyes: Up next         |
| [CloudFlare](https://developers.cloudflare.com/workers/configuration/versions-and-deployments/gradual-deployments/#_top) |         :eyes: Up next         |
| [AWS Function Aliases](https://docs.aws.amazon.com/AWSCloudFormation/latest/UserGuide/aws-resource-lambda-alias.html) |          :watch: Soon          |
|             [Kubernetes](https://kubernetes.io/)             | :hourglass_flowing_sand: Later |
| [Google Cloud Run Functions](https://cloud.google.com/functions) | :hourglass_flowing_sand: Later |

## 🎯 Mission 

We help teams take a more proactive approach to progressive delivery by using agentic deployments to catch regressions early. Today, operators either manually watch deployments in real time or rely on passive alerts to catch problems after they’ve hit users. WWe want to empower operators to *proactively* identify and rollback disruptive deployments *before* they cause widespread impact. MultiTool is bringing agentic deployments to everyone. Learn more about our team and vision at [our company website](https://www.multitool.run/company).

## 📬 Need help?

If you have questions, ideas, or bugs to report:

👉 [support@multitool.run](mailto:support@multitool.run)
