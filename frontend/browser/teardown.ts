import { request } from "@playwright/test";

export default async function teardown() {
  const context = await request.newContext();
  try {
    const response = await context.post("http://127.0.0.1:4174/__fixture/cleanup", {
      headers: { Origin: "http://127.0.0.1:4174", "x-domus-csrf": "1" },
    });
    if (!response.ok())
      throw new Error(
        `Generated fixture cleanup failed: ${response.status()} ${await response.text()}`,
      );
  } finally {
    await context.dispose();
  }
}
