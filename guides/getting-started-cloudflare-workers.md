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

- [ ] A Cloudflare account with read and write permissions for Workers Observability - read and Workers Scripts - edit

- [ ] <a href="https://developers.cloudflare.com/workers/wrangler/install-and-update/" target="_blank">Cloudflare Wrangler CLI installed</a>

  - [ ] Create an <a href="https://developers.cloudflare.com/fundamentals/api/get-started/create-token/" target="_blank">Cloudflare API Token</a>

  - [ ] Run `wrangler login` and follow the prompts to login to Cloudflare

- [ ] <a href="https://github.com/wack/multitool/releases" target="_blank">MultiTool CLI installed</a>

  - [ ] Run `multi login` to authenticate

## 📦 Step 1: Create and package the Worker code

This tutorial simulates two versions of a Worker:

- A “healthy” version that always returns a `200` HTTP status code
- A “buggy” version that randomly fails with a `400` HTTP status code 10% of the time

📝 **Note:** File **must** be named `index.js` to execute correctly.

### Create the healthy version

This version always returns a `200` HTTP status code response.

```bash
cat << EOF > index.js
exports.handler = function (_, context) {
  return context.succeed({
    statusCode: 200,
    body: JSON.stringify({
      message: "Hello World",
    }),
  });
};
EOF
```

Zip the code:

```bash
zip -j 0%_failures.zip index.js
```

### Create the buggy version

This version introduces a simulated bug by returning a `400` HTTP status code 10% of the time.

```bash
cat << EOF > index.js
exports.handler = function (_, context) {
  const rand = Math.random();
  if (rand < 0.9) {
    return context.succeed({
      statusCode: 200,
      body: JSON.stringify({
        message: "Hello World",
      }),
    });
  } else {
    return context.succeed({
      statusCode: 400,
      body: JSON.stringify({
        error: "Something went wrong",
      }),
    });
  }
};
EOF
```

Zip the code:

```bash
zip -j 10%_failures.zip index.js
```

## λ Step 2: Create the Worker

TODO: @eric continue here!

Upload the healthy version of the code to create the worker in Cloudflare:

```bash
LAMBDA_ARN=$(aws lambda create-function \
  --function-name multitool-quickstart-lambda \
  --runtime nodejs22.x \
  --handler index.handler \
  --role ${LAMBDA_EXECUTION_ROLE_ARN} \
  --zip-file fileb://0%_failures.zip \
  --publish \
  --output text \
  --query FunctionArn)
```

## 🧪 Step 4: Test that the Lambda is working

Before moving on, make sure the Lambda function returns the expected response.

```bash
aws lambda invoke --function-name multitool-quickstart-lambda out.txt >/dev/null && cat out.txt
```

You should see:

```json
{
  "statusCode": 200,
  "body": "{\"message\":\"Hello World\"}"
}
```

## ⚙️ Step 5: Set up API Gateway

Expose the Lambda to the public internet by creating an API Gateway REST API:

```bash
API_ID=$(aws apigateway create-rest-api --name multitool-quickstart-apig --output text --query id)
```

Get the auto-generated root resource ID:

```bash
ROOT_RESOURCE_ID=$(aws apigateway get-resources --rest-api-id ${API_ID} --output text --query 'items[0].id')
```

Next, create an API resource and route.

Create the new path:

```bash
RESOURCE_ID=$(aws apigateway create-resource --rest-api-id ${API_ID} --parent-id ${ROOT_RESOURCE_ID} --path-part "demo" --output text --query 'id')
```

Add a GET method:

```bash
aws apigateway put-method --rest-api-id ${API_ID} --resource-id ${RESOURCE_ID} --http-method GET --authorization-type "NONE"
```

## 🤝 Step 6: Connect API Gateway to Lambda

Link the API Gateway to the Lambda so it can forward incoming requests:

```bash
aws apigateway put-integration \
  --rest-api-id ${API_ID} \
  --resource-id ${RESOURCE_ID} \
  --http-method GET \
  --type AWS_PROXY \
  --integration-http-method POST \
  --uri arn:aws:apigateway:${AWS_REGION:=us-east-2}:lambda:path/2015-03-31/functions/${LAMBDA_ARN}/invocations
```

Deploy the API:

```bash
aws apigateway create-deployment --rest-api-id $API_ID --stage-name prod
```

Get the new public URL:

```bash
MY_URL="https://${API_ID}.execute-api.${AWS_REGION:=us-east-2}.amazonaws.com/prod/demo"
```

Save the URL to a file for later:

```bash
cat << EOF > url.txt
$MY_URL
EOF
```

Finally, give API Gateway permission to invoke the Lambda:

```bash
aws lambda add-permission \
  --function-name multitool-quickstart-lambda \
  --statement-id apigateway-permission-${API_ID} \
  --action lambda:InvokeFunction \
  --principal apigateway.amazonaws.com
```

## 🖥️ Step 7: Connect the app to MultiTool

Now that the Lambda is deployed and accessible via API Gateway, create the app in MultiTool.

From the MultiTool app:

1. Create a workspace
2. Create an application

After the application is set up, login to the MultiTool CLI if needed:

```bash
multi login
```

## ⚙️ Step 8: Add your configuration file

Now that we have our workspace and app set up in the MultiTool app, we need to create a configuration file so the MultiTool CLI knows how to deploy your application.

If you used the sample values throughout this tutorial, you can use this file:

```bash
cat << EOF > MultiTool.toml
workspace = [my_workspace_name]
application = [my_application_name]

config.monitor.aws-cloudwatch = {}

[config.ingress.aws-api-gateway]
gateway-name = "multitool-quickstart-apig"
stage-name = "prod"
resource-path = "/demo"
resource-method = "GET"
region = "us-east-2"

[config.platform.aws-lambda]
name = "multitool-quickstart-lambda"
region = "us-east-2"
EOF
```

## 🚀 Step 9: Roll out healthy code and simulate stable traffic

📝 **Note:** Exiting the terminal before a CLI operation finishes can leave your rollout in a stuck state due to a known bug. Please wait for the operation to complete before closing the terminal. If you've already run into this issue, contact support@wack.run and we’ll help resolve it. A fix is on the way.

To test a successful rollout, use the `0%_failures.zip` file.

Start the rollout using the healhty build artifact and replacing the placeholder with your MultiTool workspace name:

```bash
multi run 0%_failures.zip
```

In a separate terminal window, load the public URL from Step 6 to use in the next step:

```bash
MY_URL=$(cat url.txt)
```

Simulate traffic to the `/demo` endpoint using one of these options:

### Option A: Using curl

```bash
for i in $(seq 1 1500);do echo -n "Request $i completed with status: ";code=$(curl -s -o /dev/null -w "%{http_code}" "$MY_URL");echo "$code";sleep 1;done
```

### Option B: Using Bombardier

```bash
bombardier -c 5 -n 20 ${MY_URL}
```

As traffic hits the new version, MultiTool will evaluate its behavior and promote it to 100% traffic once it confirms stability.

## ⚠️ Step 10: Roll out buggy code and simulate errors

To test a broken rollout, use the `10%_failures.zip` file.

Start the rollout using the buggy build artifact and replacing the placeholder with your MultiTool workspace name:

```bash
multi run 10%_failures.zip
```

In a separate terminal window, load the public URL from Step 6 to use in the next step:

```bash
MY_URL=$(cat url.txt)
```

Simulate traffic to the `/demo` endpoint using one of these options:

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

## 🧹 Step 11: Cleanup

After you've tested MultiTool, be sure to clean up the demo resources created as part of this guide.

To delete the Lambda function:

```bash
aws lambda delete-function --function-name multitool-quickstart-lambda
```

and to delete the API Gateway:

```bash
aws apigateway delete-rest-api --rest-api-id ${API_ID}
```

## 📬 Need help?

If you have questions, ideas, or bugs to report:

👉 [support@multitool.run](mailto:support@multitool.run)!
