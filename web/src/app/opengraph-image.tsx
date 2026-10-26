import { alt, contentType, shareImage, size } from "@/lib/share-image";

export { alt, contentType, size };

export default function OpenGraphImage() {
  return shareImage();
}
