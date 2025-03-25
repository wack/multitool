exports.handler = function (_, context) {
  return context.succeed({
    statusCode: 200,
    body: JSON.stringify({
      message: "Hello World",
    }),
  });
};
