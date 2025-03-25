# MultiTool Getting Started Guide

In this guide, you'll learn how to safely deploy an AWS Lambda function as an API endpoint using the AWS API Gateway.

We will be creating an:

- <a href="https://aws.amazon.com/lambda/" target="_blank">AWS Lambda Function</a>
  - Lambda is a serverless compute service that runs your code in response to events and automatically manages the compute resources.
  - This guide uses Lambda to run our sample server code.
- <a href="https://aws.amazon.com/api-gateway/" target="_blank">AWS API Gateway REST API</a>
  - API Gateway is a fully managed service that makes it easy for developers to create, publish, maintain, monitor, and secure APIs at any scale.
  - This guide uses API Gateway to make our Lambda function publicly accessible to the internet.

We will be using:

- <a href="https://aws.amazon.com/cloudwatch/" target="_blank">AWS CloudWatch</a>
  - CloudWatch is a service that monitors applications in AWS.
  - This guide uses CloudWatch to read application metrics and send them to the MultiTool agent to decided whether to promote or rollback your deployments.

# Prerequisites

- [ ] <a href="https://app.multitool.run/create-account" target="_blank">Create a free MultiTool account.</a>

- [ ] Create an AWS account with at least read and write permissions for Lambda, API Gateway, and CloudWatch

- [ ] <a href="https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html" target="_blank">Install the AWS CLI.</a>

  - [ ] To login, you'll need to create an Access Key. <a href="https://docs.aws.amazon.com/IAM/latest/UserGuide/access-key-self-managed.html#Using_CreateAccessKey" target="_blank">Click here for instructions on how to create a new Access Key.</a>

  - [ ] Next, we'll use the key you created to login to the AWS CLI. Run `aws configure` and follow the prompts to login to AWS.

- [ ] <a href="https://github.com/wack/multitool/releases" target="_blank">Install the MultiTool CLI.</a>

- [ ] Login to the MultiTool CLI. Run `multi login` and follow the prompts.

---

# Getting Started with AWS Lambda and API Gateway

## Package code as a zip file

The easiest way to deploy our lambda is to package it as a zip file and upload it in the next step. We've provided 2 sample NodeJS servers that simply return a 200 or 400 HTTP response code to simulate random failures in an application.

📝 **Note:** The filename **must** be `index.js`, any other name will fail to execute correctly

Package the code with no random failures:

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

and zip the code:

```bash
zip -j 0%_failures.zip index.js
```

and package the code with 10% random failures:

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

and zip the code:

```
zip -j 10%_failures.zip index.js
```

## Create a Lambda execution IAM role

Before we create a Lambda, we need to create an IAM role that allows us to execute the function code as needed.

```bash
LAMBDA_EXECUTION_ROLE_ARN=$(aws iam create-role \
  --role-name lambda-execution \
  --assume-role-policy-document '{"Version": "2012-10-17","Statement": [{ "Effect": "Allow", "Principal": {"Service": "lambda.amazonaws.com"}, "Action": "sts:AssumeRole"}]}' --output text --query Role.Arn)
```

Alternatively, if you've already created an execution role in the past, find the Amazon Resource Name (ARN) with this command:

```bash
LAMBDA_EXECUTION_ROLE_ARN=$(aws iam get-role \
  --role-name lambda-execution \
  --output text \
  --query Role.Arn)
```

## Create a Lambda Function

After we have our code as a Zip file and an execution role created, we can now create our Lambda function.

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

## Test that our Lambda is working

Before moving on to the next step, we want to make sure our Lambda can be invoked and returns the correct response.

```bash
aws lambda invoke --function-name multitool-quickstart-lambda out.txt >/dev/null && cat out.txt
```

Your output should look like this:

```json
{
  "statusCode": 200,
  "body": "{\"message\":\"Hello World\"}"
}
```

## Create an API Gateway

After we have successfully created our Lambda function and tested it, we'll need to create and set up an API Gateway so we can invoke our function from a public API endpoint.

```bash
API_ID=$(aws apigateway create-rest-api --name multitool-quickstart-apig --output text --query id)
```

## Get the Root resource's auto-generated ID

Next, we need to get the auto-generated `/` endpoint's resource ID to use it in the next command.

```bash
ROOT_RESOURCE_ID=$(aws apigateway get-resources --rest-api-id ${API_ID} --output text --query 'items[0].id')
```

## Create a resource in the gateway

Now that we have the root resource's id, we need to create a new resource. In this case, it's a new endpoint called `/demo`.

```bash
RESOURCE_ID=$(aws apigateway create-resource --rest-api-id ${API_ID} --parent-id ${ROOT_RESOURCE_ID} --path-part "demo" --output text --query 'id')
```

## Add a GET endpoint method to the resource

Now that we've created our new `/demo` resource, we need to assign it to a `GET` request.

```bash
aws apigateway put-method --rest-api-id ${API_ID} --resource-id ${RESOURCE_ID} --http-method GET --authorization-type "NONE"
```

## Update the API Gateway to point at the Lambda we created

Finally, we can point our new resource to our lambda, which will enable us to invoke the lambda from calling the API Gateway endpoint.

```bash
aws apigateway put-integration \
    --rest-api-id ${API_ID} \
    --resource-id ${RESOURCE_ID} \
    --http-method GET \
    --type AWS_PROXY \
    --integration-http-method POST \
    --uri arn:aws:apigateway:${AWS_REGION:=us-east-2}:lambda:path/2015-03-31/functions/${LAMBDA_ARN}/invocations
```

## Create an API Gateway deployment

When updating the integration, we need to create a new deployment.

```bash
aws apigateway create-deployment --rest-api-id $API_ID --stage-name prod
```

## Get our API Gateway URL

```bash
MY_URL="https://${API_ID}.execute-api.${AWS_REGION:=us-east-2}.amazonaws.com/prod/demo"
```

and save that url as a file that we can use later:

```bash
cat << EOF > url.txt
$MY_URL
EOF
```

## Give permissions for the API Gateway to invoke the Lambda

```bash
aws lambda add-permission \
    --function-name multitool-quickstart-lambda \
    --statement-id apigateway-permission-${API_ID} \
    --action lambda:InvokeFunction \
    --principal apigateway.amazonaws.com
```

---

# Getting Started with MultiTool

Now that we have a Lambda function and an API Gateway that can be publicly accessed, we can simulate a buggy update by pushing updated lambda code that randomly returns an error some percent of the time.

## Setup your application

Once you have logged into the MultiTool dashboard, create a workspace, then create an application using these values. If you updated any of the values in the steps above, be sure to use the updated values when creating your application.

| Name                  | Value                           |
| --------------------- | ------------------------------- |
| Application Name      | **quickstart-app**              |
| Region                | **us-east-2**                   |
| REST API gateway name | **multitool-quickstart-apig**   |
| Gateway stage         | **prod**                        |
| Resource method       | **GET**                         |
| Resource path         | **/demo**                       |
| Lambda name           | **multitool-quickstart-lambda** |

## Login with the MultiTool CLI

Once you've created your application, run the login command to connect the MultiTool CLI with your dashboard.

```bash
multi login
```

## Deploy using MultiTool and Simulate Traffic

Use `multi run` to upload your updated lambda code and start the canary deployment.

If you want to test a scenario that causes a **deployment**, deploy the `0%_failures.zip` code with MultiTool.

If you want to test a scenario that causes a **rollback**, deploy the `10%_failures.zip` code with MultiTool.

In order to see MultiTool in action, we'll need to simulate some traffic to our `/demo` endpoint. We can do that automatically with a tool like [Bombardier](https://github.com/codesenberg/bombardier) or manually with an API Client like `curl`, [Insomnia](https://insomnia.rest/), or [Postman](https://www.postman.com/).

1. Start your deployment

```bash
multi run --workspace ${MY_WORKSPACE_NAME} --application ${MY_APPLICATION_NAME} 0%_failures.zip
```

2. In a new terminal window, simulate traffic

In a new terminal window, first pull your url from the file we previously saved:

```bash
MY_URL=$(cat url.txt)
```

then run one of these commands to start sending traffic to our endpoint:

```bash
# Using curl
for i in $(seq 1 1500);do echo -n "Request $i completed with status: ";code=$(curl -s -o /dev/null -w "%{http_code}" "$MY_URL");echo "$code";sleep 1;done

OR

# Using bombardier
bombardier -c 5 -n 20 ${MY_URL}
```

And that's it 🎉 Your code will be automatically deployed and progressively released while testing its stability.

---

# Support

If you have any questions, ideas, or bugs to report, please reach out to [support@multitool.run](mailto:support@multitool.run)!
