# Required AWS permissions for MultiTool

MultiTool is designed to minimize security risk. <b>MultiTool never stores AWS credentials</b> and relies solely on an authenticated AWS CLI session for access.

This document lists the minimum set of permissions required for MultiTool to operate end-to-end, including creating AWS resources and running deployments.

## Minimum IAM policy

💡 To create a new policy in the AWS console, <a href="https://docs.aws.amazon.com/IAM/latest/UserGuide/access_policies_create-console.html#access_policies_create-json-editor" target="_blank">follow these instructions</a>.

The following IAM policy defines the least privilege access MultiTool needs to function correctly:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "multitoolminimumpermissions",
      "Effect": "Allow",
      "Action": [
        "apigateway:*",
        "apigateway:AddCertificateToDomain",
        "apigateway:CreateAccessAssociation",
        "apigateway:DELETE",
        "apigateway:GET",
        "apigateway:PATCH",
        "apigateway:POST",
        "apigateway:PUT",
        "apigateway:RejectAccessAssociation",
        "apigateway:RemoveCertificateFromDomain",
        "apigateway:SetWebACL",
        "apigateway:UpdateDomainNameManagementPolicy",
        "apigateway:UpdateDomainNamePolicy",
        "apigateway:UpdateRestApiPolicy",
        "cloudwatch:CreateServiceLevelObjective",
        "cloudwatch:DeleteMetricStream",
        "cloudwatch:DescribeAlarmHistory",
        "cloudwatch:DescribeAlarms",
        "cloudwatch:DescribeAlarmsForMetric",
        "cloudwatch:DescribeAnomalyDetectors",
        "cloudwatch:DescribeInsightRules",
        "cloudwatch:GenerateQuery",
        "cloudwatch:GetDashboard",
        "cloudwatch:GetInsightRuleReport",
        "cloudwatch:GetMetricData",
        "cloudwatch:GetMetricStatistics",
        "cloudwatch:GetMetricStream",
        "cloudwatch:GetMetricWidgetImage",
        "cloudwatch:GetService",
        "cloudwatch:GetServiceData",
        "cloudwatch:GetTopologyDiscoveryStatus",
        "cloudwatch:GetTopologyMap",
        "cloudwatch:ListDashboards",
        "cloudwatch:ListEntitiesForMetric",
        "cloudwatch:ListManagedInsightRules",
        "cloudwatch:ListMetrics",
        "cloudwatch:ListMetricStreams",
        "cloudwatch:ListServiceLevelObjectives",
        "cloudwatch:ListServices",
        "cloudwatch:ListTagsForResource",
        "cloudwatch:PutDashboard",
        "cloudwatch:PutMetricAlarm",
        "cloudwatch:PutMetricData",
        "cloudwatch:PutMetricStream",
        "cloudwatch:StartMetricStreams",
        "cloudwatch:StopMetricStreams",
        "cloudwatch:TagResource",
        "cloudwatch:UntagResource",
        "iam:CreateAccessKey",
        "iam:CreateRole",
        "iam:DeleteAccessKey",
        "iam:GetRole",
        "iam:ListAccessKeys",
        "iam:PassRole",
        "iam:UpdateAccessKey",
        "lambda:AddPermission",
        "lambda:CreateAlias",
        "lambda:CreateEventSourceMapping",
        "lambda:CreateFunction",
        "lambda:CreateFunctionUrlConfig",
        "lambda:DeleteAlias",
        "lambda:DeleteEventSourceMapping",
        "lambda:DeleteFunction",
        "lambda:DeleteFunctionEventInvokeConfig",
        "lambda:DeleteFunctionUrlConfig",
        "lambda:DisableReplication",
        "lambda:EnableReplication",
        "lambda:GetAccountSettings",
        "lambda:GetAlias",
        "lambda:GetEventSourceMapping",
        "lambda:GetFunction",
        "lambda:GetFunctionConfiguration",
        "lambda:GetFunctionEventInvokeConfig",
        "lambda:GetFunctionUrlConfig",
        "lambda:GetPolicy",
        "lambda:GetRuntimeManagementConfig",
        "lambda:InvokeAsync",
        "lambda:InvokeFunction",
        "lambda:InvokeFunctionUrl",
        "lambda:ListAliases",
        "lambda:ListEventSourceMappings",
        "lambda:ListFunctionEventInvokeConfigs",
        "lambda:ListFunctions",
        "lambda:ListFunctionUrlConfigs",
        "lambda:ListTags",
        "lambda:ListVersionsByFunction",
        "lambda:PublishVersion",
        "lambda:PutFunctionEventInvokeConfig",
        "lambda:PutRuntimeManagementConfig",
        "lambda:RemovePermission",
        "lambda:UpdateAlias",
        "lambda:UpdateEventSourceMapping",
        "lambda:UpdateFunctionCode",
        "lambda:UpdateFunctionConfiguration",
        "lambda:UpdateFunctionEventInvokeConfig"
      ],
      "Resource": "*"
    }
  ]
}
```

## 📬 Need help?

If you have questions, ideas, or bugs to report:

👉 [support@multitool.run](mailto:support@multitool.run)
