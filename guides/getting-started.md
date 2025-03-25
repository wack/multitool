# Getting started: Deploy a Lambda function with MultiTool

This tutorial walks through deploying a simple AWS Lambda function behind an API Gateway. You’ll simulate user traffic to the API, and the MultiTool agent will automatically decide whether to promote or roll back the deployment based on the observed error rate.

You will:

1. Create and package sample Lambda code
2. Deploy to AWS with API Gateway
3. Connect to MultiTool and run a canary deployment

## 🛠 Tools

- <a href="https://aws.amazon.com/lambda/" target="_blank">AWS Lambda</a> - to run sample server code

- <a href="https://aws.amazon.com/api-gateway/" target="_blank">AWS API Gateway REST API</a> - to make the Lambda function publicly accessible

- <a href="https://aws.amazon.com/cloudwatch/" target="_blank">AWS CloudWatch</a> - to read metrics used by MultiTool during deployment

- <a href="https://app.multitool.run/create-account" target="_blank">MultiTool</a> - to automate safe deployments

## ✅ Prerequisites

- [ ] <a href="https://app.multitool.run/create-account" target="_blank">A free MultiTool account</a>

- [ ] An AWS account with read and write permissions for Lambda, API Gateway, and CloudWatch

- [ ] <a href="https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html" target="_blank">AWS CLI installed</a>

  - [ ] Create an <a href="https://docs.aws.amazon.com/IAM/latest/UserGuide/access-key-self-managed.html#Using_CreateAccessKey" target="_blank">Access Key</a>

  - [ ] Run `aws configure` and follow the prompts to login to AWS

- [ ] <a href="https://github.com/wack/multitool/releases" target="_blank">MultiTool CLI installed</a>

  - [ ] Run `multi login` to authenticate

## 📦 Step 1: Create and package the Lambda code

This tutorial simulates two versions of a Lambda function:

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

```
zip -j 10%_failures.zip index.js
```

## ➕ Step 2: Create a Lambda execution IAM role

AWS needs permission to run the Lambda function.

Create a new IAM role for Lambda execution:

```bash
LAMBDA_EXECUTION_ROLE_ARN=$(aws iam create-role \
--role-name lambda-execution \
--assume-role-policy-document '{"Version": "2012-10-17","Statement": [{ "Effect": "Allow", "Principal": {"Service": "lambda.amazonaws.com"}, "Action": "sts:AssumeRole"}]}' --output text --query Role.Arn)
```

If you’ve already created this role before, you can retrieve it instead:

```bash
LAMBDA_EXECUTION_ROLE_ARN=$(aws iam get-role \
--role-name lambda-execution \
--output text \
--query Role.Arn)
```

## λ Step 3: Create the Lambda function

Upload the healthy version of the code to create the function in AWS:

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

From the MultiTool web dashboard:

1. Create a workspace
2. Create an application with the following values:

| Name                  | Value                           |
| --------------------- | ------------------------------- |
| Application Name      | **quickstart-app**              |
| Region                | **us-east-2**                   |
| REST API gateway name | **multitool-quickstart-apig**   |
| Gateway stage         | **prod**                        |
| Resource method       | **GET**                         |
| Resource path         | **/demo**                       |
| Lambda name           | **multitool-quickstart-lambda** |

After the application is set up, login to the MultiTool CLI if needed:

```bash
multi login
```

## 🚢 Step 8: Deploy with MultiTool and simulate traffic

Deploy either version of the code using MultiTool:

- To test a healthy deployment, use the `0%_failures.zip` file.
- To test a buggy deployment, use the `10%_failures.zip` file.

Both will be progressively released and MultiTool will promote the deployment if the app is stable, or roll back if error rates spike.

Start the deployment by replacing the placeholder with your MultiTool workspace name:

```bash
multi run --workspace ${MY_WORKSPACE_NAME} --application quickstart-app 0%_failures.zip
```

This kicks off a deployment and tells MultiTool to begin gradually shifting traffic to the new version.

Now, in a separate terminal window, load the public URL from Step 6 to use in the next step:

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

When deploying the buggy version (`10%failures.zip`), MultiTool will automatically roll back. When deploying the healthy version (`0%_failures.zip`), MultiTool will fully promote the new version.

And that’s it! 🎉

## 📬 Need help?

If you have questions, ideas, or bugs to report:

👉 [support@multitool.run](mailto:support@multitool.run)!
