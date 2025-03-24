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
