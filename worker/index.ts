export default {
  async fetch(request, env, ctx): Promise<Response> {
    const rand = Math.random();

    const response = new Response(
      rand < 0.5 ? "Bad Request with changes" : "Hello World With Changes!",
      { status: rand < 0.5 ? 400 : 200 }
    );

    response.headers.set("Canary", "true");

    response.headers.set("Access-Control-Allow-Origin", "*");
    response.headers.set("Access-Control-Allow-Methods", "*");
    response.headers.set("Access-Control-Expose-Headers", "*");

    console.log("These are sopme more changes");

    return response;
  },
} satisfies ExportedHandler<Env>;
