# Getting started: Roll out a Cloudflare Worker with MultiTool

This tutorial walks through rolling out a simple Cloudflare Worker. You’ll simulate user traffic to the API, and the MultiTool agent will automatically decide whether to promote or roll out based on the observed error rate.

You will:

1. Create and package sample Worker code
2. Deploy to Cloudflare
3. Connect to MultiTool and run a canary rollout

## 🛠 Tools

- <a href="https://workers.cloudflare.com/" target="_blank">Cloudflare Workers</a> - to run sample server code

- <a href="https://developers.cloudflare.com/workers/observability/" target="_blank">Cloudflare Workers Observability</a> - to read metrics used by MultiTool during rollout

- <a href="https://app.multitool.run/create-account" target="_blank">MultiTool</a> - to automate safe rollouts

## ✅ Prerequisites

📝 **Note:** This tutorial is compatible with macOS and Linux systems. For Windows users, we recommend using <a href="https://learn.microsoft.com/en-us/windows/wsl/about" target="_blank">Windows Subsystem for Linux (WSL)</a>.

- [ ] <a href="https://app.multitool.run/create-account" target="_blank">A free MultiTool account</a>

- [ ] A Cloudflare account and token that has permissions for Workers Observability - read and Workers Scripts - edit

- [ ] <a href="https://developers.cloudflare.com/workers/wrangler/install-and-update/" target="_blank">Cloudflare Wrangler CLI installed</a>

  - [ ] Create an <a href="https://developers.cloudflare.com/fundamentals/api/get-started/create-token/" target="_blank">Cloudflare API Token</a> with Workers Observability - read and Workers Scripts - edit permissions.

  - [ ] Run `npx wrangler login` and follow the prompts to login to Cloudflare

- [ ] <a href="https://github.com/wack/multitool/releases" target="_blank">MultiTool CLI installed</a>

  - [ ] Run `multi login` to authenticate

## 🏗️ Step 1: Create the Worker

Create a new "Hello World" Cloudflare Worker

```bash
npm create cloudflare@latest -- multitool-quickstart --type hello-world --lang ts --no-git -y true
```

## 📦 Step 2: Create and package the Worker code

This tutorial simulates two versions of a Worker:

- A “healthy” version that always returns a `200` HTTP status code
- A “buggy” version that randomly fails with a `400` HTTP status code 50% of the time

First, let's enter the workers `src` directory:

```bash
cd multitool-quickstart/src/
```

Overwrite the `index.js` file and add a new file for the healthy and buggy vsions:

### Create the healthy version

This version always returns a `200` HTTP status code response.

```bash
cat << EOF > index.ts
export default {
	async fetch(request, env, ctx): Promise<Response> {
		return new Response('Hello World!', { status: 200 });
	},
} satisfies ExportedHandler<Env>;
EOF
```

### Create the buggy version

This version introduces a simulated bug by returning a `400` HTTP status code 50% of the time.

```bash
cat << EOF > index_errors.ts
export default {
	async fetch(request, env, ctx): Promise<Response> {
		const rand = Math.random();
		return new Response(rand < 0.5 ? 'Bad Request' : 'Hello World!', { status: rand < 0.5 ? 400 : 200 });
	},
} satisfies ExportedHandler<Env>;
EOF
```

Finally, we can go back to the root directory of our Worker:

```bash
cd ..
```

## ⚙️ Step 3: Deploy the worker

Now that we added the updated code to our worker, let's deploy it.

```bash
npx wrangler deploy
```

Make sure to store the URL that looks like this, replacing `MY_ACCOUNT_URL` with the value from the output of the `deploy` command:

```bash
MY_URL="https://multitool-quickstart.[MY_ACCOUNT_URL].workers.dev"
```

And save the URL to a file for later:

```bash
cat << EOF > url.txt
$MY_URL
EOF
```

## 🧪 Step 4: Test that the Worker is accepting traffic

Before moving on, make sure the Worker returns the expected response.

```bash
curl $MY_URL
```

You should see:

```bash
Hello World!
```

## 🖥️ Step 5: Connect the app to MultiTool

Now that the Worker is deployed and accessible via its URL, create the application in MultiTool.

From the MultiTool app:

1. Create a workspace
2. Create an application named `quickstart`

After the application is set up, login to the MultiTool CLI if needed:

```bash
multi login
```

## ⚙️ Step 6: Add your configuration file

Now that we have our workspace and app set up in the MultiTool app, we need to create a manfiest file called `MultiTool.toml` so the MultiTool CLI knows how to deploy your application.

If you used the sample values throughout this tutorial, you can use this file, but make sure to replace MY_WORKSPACE_NAME, and MY_CLOUDFLARE_ACCOUNT_ID with the correct values:

📝 **Note:** To get your Cloudflare Account ID, [follow the instructions here](https://developers.cloudflare.com/fundamentals/account/find-account-and-zone-ids/).

```bash
cat << EOF > MultiTool.toml
workspace = "MY_WORKSPACE_NAME"
application = "quickstart"

[config.cloudflare]
worker-name = "multitool-quickstart"
account-id = "MY_CLOUDFLARE_ACCOUNT_ID"
main-module = "index.ts"
artifact-path = "src/"
EOF
```

## 🚀 Step 7: Roll out healthy code and simulate stable traffic

📝 **Note:** Exiting the terminal before a CLI operation finishes can leave your rollout in a stuck state due to a known bug. Please wait for the operation to complete before closing the terminal. If you've already run into this issue, contact support@wack.run and we’ll help resolve it. A fix is on the way.

Start the rollout using `index.ts` as the `main-module` value in your `MultiTool.toml` file:

```bash
multi run
```

In a separate terminal window, load the public URL from Step 6 to use in the next step:

```bash
MY_URL=$(cat url.txt)
```

Simulate traffic to the worker using one of these options:

### Option A: Using curl

```bash
for i in $(seq 1 1500);do echo -n "Request $i completed with status: ";code=$(curl -s -o /dev/null -w "%{http_code}" "$MY_URL");echo "$code";sleep 1;done
```

### Option B: Using Bombardier

```bash
bombardier -c 5 -n 20 ${MY_URL}
```

As traffic hits the new version, MultiTool will evaluate its behavior and promote it to 100% traffic once it confirms stability.

## ⚠️ Step 8: Roll out buggy code and simulate errors

To test a broken rollout, use the `index_errors.ts` file.

Start the rollout using `index_errors.ts` as the `main-module` value in your `MultiTool.toml` file:

```bash
multi run
```

In a separate terminal window, load the public URL from Step 6 to use in the next step:

```bash
MY_URL=$(cat url.txt)
```

Simulate traffic to the Worker using one of these options:

### Option A: Using curl

```bash
for i in $(seq 1 1500);do echo -n "Request $i completed with status: ";code=$(curl -s -o /dev/null -w "%{http_code}" "$MY_URL");echo "$code";sleep 1;done
```

### Option B: Using Bombardier

```bash
bombardier -c 5 -n 20 ${MY_URL}
```

MultiTool will detect the increase in errors and automatically trigger a rollback.

And that’s it! 🎉

## 🧹 Step 9: Cleanup

After you've tested MultiTool, be sure to clean up the Worker created as part of this guide:

```bash
npx wrangler delete multitool-quickstart
```

## 📬 Need help?

If you have questions, ideas, or bugs to report:

👉 [support@multitool.run](mailto:support@multitool.run)!
