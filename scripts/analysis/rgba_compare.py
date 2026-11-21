"""Compare matching RGBA captures and measure near-black background coverage."""

import argparse
import json
import math
from pathlib import Path

from PIL import Image, ImageChops, ImageFilter


def count_nonzero(image: Image.Image) -> int:
    return image.width * image.height - image.histogram()[0]


def compare(reference: Image.Image, candidate: Image.Image) -> dict:
    if reference.size != candidate.size:
        raise ValueError(f"image sizes differ: {reference.size} vs {candidate.size}")
    delta = ImageChops.difference(reference, candidate)
    channels = delta.split()
    maximum = ImageChops.lighter(ImageChops.lighter(channels[0], channels[1]),
                                 ImageChops.lighter(channels[2], channels[3]))
    alpha = reference.getchannel("A")
    edge = ImageChops.difference(alpha.filter(ImageFilter.MaxFilter(3)),
                                 alpha.filter(ImageFilter.MinFilter(3)))
    interior = maximum.copy()
    interior.paste(0, mask=edge.point(lambda value: 255 if value else 0))
    changed = count_nonzero(maximum)
    outside = count_nonzero(interior)
    pixels = reference.width * reference.height
    squared_error = sum(
        value * value * count
        for channel in channels
        for value, count in enumerate(channel.histogram())
    )
    return {
        "pixels": pixels,
        "different_pixels": changed,
        "different_fraction": changed / pixels,
        "max_channel_delta": maximum.getextrema()[1],
        "alpha_edge_fraction_of_differences": (changed - outside) / changed if changed else 1.0,
        "outside_alpha_edge_pixels": outside,
        "outside_alpha_edge_max_delta": interior.getextrema()[1],
        "psnr_db": 10 * math.log10(255**2 * pixels * 4 / squared_error) if squared_error else None,
    }


def near_black_fraction(image: Image.Image, top_fraction: float, threshold: int) -> float:
    height = max(1, int(image.height * top_fraction))
    red, green, blue = image.crop((0, 0, image.width, height)).convert("RGB").split()
    brightest = ImageChops.lighter(ImageChops.lighter(red, green), blue)
    return sum(brightest.histogram()[:threshold + 1]) / (image.width * height)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--top-fraction", type=float, default=0.6)
    parser.add_argument("--black-threshold", type=int, choices=range(256), default=8)
    args = parser.parse_args()
    if not 0 < args.top_fraction <= 1:
        parser.error("--top-fraction must be in (0, 1]")
    with Image.open(args.reference) as image:
        reference = image.convert("RGBA")
    with Image.open(args.candidate) as image:
        candidate = image.convert("RGBA")
    result = compare(reference, candidate)
    result["reference_near_black_fraction"] = near_black_fraction(
        reference, args.top_fraction, args.black_threshold,
    )
    result["candidate_near_black_fraction"] = near_black_fraction(
        candidate, args.top_fraction, args.black_threshold,
    )
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
