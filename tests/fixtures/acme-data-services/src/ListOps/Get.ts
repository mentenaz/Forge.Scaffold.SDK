import { spfi, SPFx } from "@pnp/sp";
import "@pnp/sp/lists";
import { uniqBy } from "lodash-es";
import { ServiceContext } from "../ctx";
import { messageOf } from "../errors";

export const getLists = async ({ context }: ServiceContext) => {
  try {
    const lists = await spfi().using(SPFx(context)).web.lists();
    return uniqBy(lists, "Id");
  } catch (err) {
    throw messageOf(err, "An unknown error occurred while fetching the lists.");
  }
};
