import { spfi, SPFx } from "@pnp/sp";
import "@pnp/sp/items";
import { ServiceContext } from "../ctx";
import { messageOf } from "../errors";

export const createItem = async ({ context, list, data }: ServiceContext & { list: string; data: Record<string, unknown> }) => {
  try {
    return await spfi().using(SPFx(context)).web.lists.getByTitle(list).items.add(data);
  } catch (err) {
    throw messageOf(err, "An unknown error occurred while creating the item.");
  }
};
