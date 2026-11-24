import { spfi, SPFx } from "@pnp/sp";
import "@pnp/sp/webs";
import "@pnp/sp/lists";
import "@pnp/sp/items";
import { formatISO } from "date-fns";
import { z } from "zod";
import { ServiceContext, siteOf } from "../ctx";
import { messageOf } from "../errors";
import type { ItemRef } from "../types";

const Reply = z.object({ Id: z.number(), Title: z.string().optional() });

export const getItem = async ({ context, list, id }: ServiceContext & ItemRef): Promise<{ Id: number; Title?: string; at: string }> => {
  try {
    const item = await spfi(siteOf({ context })).using(SPFx(context)).web.lists.getByTitle(list).items.getById(id)();
    return { ...Reply.parse(item), at: formatISO(new Date()) };
  } catch (err) {
    throw messageOf(err, "An unknown error occurred while fetching the item.");
  }
};
